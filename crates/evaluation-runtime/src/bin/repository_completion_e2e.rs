//! Run a frozen repository-completion bundle through the real daemon.
//!
//! The model is allowed to edit a fresh fixture only through the daemon's
//! normal PawGate/Claw path. The current contract is a cargo-fixture E2E smoke:
//! it scores a copied, settled worktree with an isolated cargo validation
//! command and appends a hash-chained JSONL record. It does not fetch real
//! benchmark repositories or claim that fixture-visible tests are held out.
//! Frozen tasks use `variant=static` (no correction) or `variant=correction`
//! (one correction after the first successful non-empty mutation event).
//! A task with a durable start but no terminal record is marked uncertain on
//! resume and is never replayed.

use purrcode_runtime_core::{FilesystemScope, SessionEvent};
use reqwest::StatusCode;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Component, Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const SCHEMA_VERSION: u32 = 1;
const HTTP_CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const HTTP_TIMEOUT: Duration = Duration::from_secs(15);
const POLL_INTERVAL: Duration = Duration::from_millis(250);
const STOP_CONFIRM_TIMEOUT: Duration = Duration::from_secs(15);
const CORRECTION_ACK_TIMEOUT: Duration = Duration::from_secs(5);
const DEFAULT_VALIDATION_TIMEOUT: u64 = 60;
const MAX_TASK_SECONDS: u64 = 15 * 60;
const MAX_VALIDATION_SECONDS: u64 = 10 * 60;
const MAX_COPY_FILES: usize = 100_000;
const MAX_COPY_BYTES: u64 = 512 * 1024 * 1024;

#[derive(Debug)]
struct Options {
    daemon: String,
    token: String,
    bundle: PathBuf,
    events: PathBuf,
    results: PathBuf,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Bundle {
    schema_version: u32,
    study_id: String,
    model: String,
    plan_sha256: String,
    freeze_sha256: String,
    dataset_manifest_sha256: String,
    schedule_sha256: String,
    tasks: Vec<Task>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Task {
    schedule_index: u64,
    run_id: String,
    task_id: String,
    repository: String,
    base_task_id: String,
    variant: String,
    arm: String,
    repeat: u64,
    candidate: String,
    condition: String,
    alignment: String,
    objective: String,
    fixture: PathBuf,
    fixture_sha256: String,
    expected_changed_paths: Vec<String>,
    #[serde(default)]
    forbidden_paths: Vec<String>,
    #[serde(default)]
    max_tokens: Option<u64>,
    #[serde(default)]
    mid_run_correction: Option<String>,
    maximum_seconds: u64,
    validation: Validation,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Validation {
    program: String,
    #[serde(alias = "args")]
    arguments: Vec<String>,
    #[serde(default = "default_validation_timeout")]
    timeout_seconds: u64,
}

fn default_validation_timeout() -> u64 {
    DEFAULT_VALIDATION_TIMEOUT
}

#[derive(Debug)]
struct LoadedBundle {
    bundle: Bundle,
    bundle_sha256: String,
    bundle_path: PathBuf,
}

#[derive(Debug)]
struct TreeSnapshot {
    files: BTreeMap<String, String>,
    sha256: String,
}

#[derive(Debug, Serialize)]
struct TaskMeasurement {
    schedule_index: u64,
    run_id: String,
    task_id: String,
    repository: String,
    base_task_id: String,
    variant: String,
    arm: String,
    repeat: u64,
    candidate: String,
    condition: String,
    model: String,
    plan_sha256: String,
    freeze_sha256: String,
    dataset_manifest_sha256: String,
    schedule_sha256: String,
    mid_run_correction_requested: bool,
    mid_run_correction_triggered: bool,
    mid_run_correction_acknowledged: bool,
    mid_run_correction_trigger_event_sha256: Option<String>,
    mid_run_correction_message_event_sha256: Option<String>,
    alignment_requested: String,
    alignment_observed: Option<bool>,
    terminal_status: &'static str,
    oracle_status: &'static str,
    externally_correct: bool,
    claimed_complete: bool,
    status: &'static str,
    session_id: String,
    session_status: String,
    timed_out: bool,
    elapsed_ms: u128,
    validation_passed: bool,
    validation_elapsed_ms: u128,
    baseline_tree_sha256: String,
    scored_tree_sha256: String,
    changed_paths: Vec<String>,
    missing_expected_paths: Vec<String>,
    forbidden_changed_paths: Vec<String>,
    model_calls: u64,
    incomplete_model_calls: u64,
    input_tokens: u64,
    output_tokens: u64,
    providers: Vec<String>,
    models: Vec<String>,
    session_events: u64,
    session_events_sha256: String,
}

#[derive(Debug)]
struct SessionMeasurement {
    session_id: String,
    session_status: String,
    timed_out: bool,
    events: Vec<SessionEvent>,
    worktree: PathBuf,
    elapsed: Duration,
    correction: CorrectionMeasurement,
}

#[derive(Debug, Default)]
struct CorrectionMeasurement {
    requested: bool,
    triggered: bool,
    acknowledged: bool,
    trigger_event_sha256: Option<String>,
    message_event_sha256: Option<String>,
}

#[derive(Debug)]
struct CorrectionTrigger {
    event_sha256: String,
    affected_path_count: usize,
    minimum_event_index: usize,
}

/// Append-only, hash-chained evidence. The event log intentionally contains
/// no objective, response, headers, or credential material.
struct EvidenceLog {
    file: File,
    study_id: String,
    bundle_sha256: String,
    sequence: u64,
    previous_hash: Option<String>,
    started: BTreeSet<String>,
    terminal: BTreeMap<String, String>,
    terminal_records: BTreeMap<String, Value>,
    task_ids: BTreeSet<String>,
    correction_sent: BTreeSet<String>,
    correction_acknowledged: BTreeSet<String>,
}

struct ResultLog {
    file: File,
    records: BTreeMap<String, Value>,
}

fn usage() -> ! {
    eprintln!(
        "usage: purrcode-repository-completion-e2e \
         --daemon URL --token-file FILE --bundle FILE --events FILE [--results FILE]\n\
         events is the immutable audit chain; pass results JSONL to fse_confirmatory_analysis"
    );
    std::process::exit(2)
}

fn parse_options() -> Options {
    let mut daemon = None;
    let mut token_file = None;
    let mut bundle = None;
    let mut events = None;
    let mut results = None;
    let mut arguments = std::env::args().skip(1);
    while let Some(flag) = arguments.next() {
        let mut value = || arguments.next().unwrap_or_else(|| usage());
        match flag.as_str() {
            "--daemon" => daemon = Some(value()),
            "--token-file" => token_file = Some(PathBuf::from(value())),
            "--bundle" => bundle = Some(PathBuf::from(value())),
            "--events" => events = Some(PathBuf::from(value())),
            "--results" => results = Some(PathBuf::from(value())),
            "-h" | "--help" => usage(),
            _ => usage(),
        }
    }
    let token_path = token_file.unwrap_or_else(|| usage());
    let token = fs::read_to_string(&token_path)
        .unwrap_or_else(|error| {
            eprintln!("could not read daemon token file: {error}");
            std::process::exit(2)
        })
        .trim()
        .to_owned();
    if token.is_empty() || token.chars().any(char::is_control) {
        eprintln!("daemon token file is empty or contains control characters");
        std::process::exit(2)
    }
    let daemon = daemon.unwrap_or_else(|| usage());
    validate_daemon_url(&daemon).unwrap_or_else(|error| {
        eprintln!("invalid daemon URL: {error}");
        std::process::exit(2)
    });
    let events = events.unwrap_or_else(|| usage());
    let results = results.unwrap_or_else(|| {
        let mut path = events.clone();
        path.set_extension("results.jsonl");
        path
    });
    Options {
        daemon,
        token,
        bundle: bundle.unwrap_or_else(|| usage()),
        events,
        results,
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let options = parse_options();
    let loaded = load_bundle(&options.bundle)?;
    let task_ids = loaded
        .bundle
        .tasks
        .iter()
        .map(|task| task.task_id.clone())
        .collect::<BTreeSet<_>>();
    let mut evidence = EvidenceLog::open(
        &options.events,
        &loaded.bundle.study_id,
        &loaded.bundle_sha256,
        task_ids,
    )?;
    let mut results = ResultLog::open(&options.results)?;
    results.sync_from_evidence(&evidence, &loaded.bundle.tasks, &loaded.bundle)?;
    let client = reqwest::Client::builder()
        .connect_timeout(HTTP_CONNECT_TIMEOUT)
        .timeout(HTTP_TIMEOUT)
        .build()?;

    for task in &loaded.bundle.tasks {
        if evidence.is_terminal(&task.task_id) {
            eprintln!("↪ {} already has a terminal record", task.task_id);
            continue;
        }
        eprintln!("▶ {}", task.task_id);
        evidence.append_started(task, &loaded.bundle.model)?;
        let result = execute_task(&client, &options, &loaded, task, &mut evidence).await;
        match result {
            Ok(measurement) => {
                let status = measurement.status;
                evidence.append_terminal(
                    "task_finished",
                    &task.task_id,
                    serde_json::to_value(measurement)?,
                )?;
                results.sync_from_evidence(&evidence, &loaded.bundle.tasks, &loaded.bundle)?;
                eprintln!("  {status}");
            }
            Err(error) => {
                let detail = redact_error(&error.to_string());
                let mut fields = task_result_identity(task, &loaded.bundle);
                fields.insert("status".into(), json!("infrastructure_error"));
                fields.insert("terminal_status".into(), json!("error"));
                fields.insert("oracle_status".into(), json!("unknown"));
                fields.insert(
                    "mid_run_correction_triggered".into(),
                    json!(evidence.correction_sent.contains(&task.task_id)),
                );
                fields.insert(
                    "mid_run_correction_acknowledged".into(),
                    json!(evidence.correction_acknowledged.contains(&task.task_id)),
                );
                fields.insert(
                    "mid_run_correction_trigger_event_sha256".into(),
                    Value::Null,
                );
                fields.insert(
                    "mid_run_correction_message_event_sha256".into(),
                    Value::Null,
                );
                fields.insert("externally_correct".into(), Value::Null);
                fields.insert("claimed_complete".into(), Value::Null);
                fields.insert("detail".into(), json!(detail));
                evidence.append_terminal("task_error", &task.task_id, Value::Object(fields))?;
                results.sync_from_evidence(&evidence, &loaded.bundle.tasks, &loaded.bundle)?;
                eprintln!("  infrastructure_error");
            }
        }
    }

    if evidence.terminal.values().any(|status| status != "passed")
        || evidence.terminal.len() != evidence.task_ids.len()
    {
        std::process::exit(1)
    }
    Ok(())
}

fn validate_daemon_url(raw: &str) -> Result<(), String> {
    let url = reqwest::Url::parse(raw).map_err(|error| error.to_string())?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err("scheme must be http or https".into());
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err("URL userinfo is not allowed".into());
    }
    Ok(())
}

fn task_result_identity(task: &Task, bundle: &Bundle) -> Map<String, Value> {
    let mut fields = Map::new();
    fields.insert("study_id".into(), json!(bundle.study_id));
    fields.insert("schedule_index".into(), json!(task.schedule_index));
    fields.insert("run_id".into(), json!(task.run_id));
    fields.insert("task_id".into(), json!(task.task_id));
    fields.insert("repository".into(), json!(task.repository));
    fields.insert("base_task_id".into(), json!(task.base_task_id));
    fields.insert("variant".into(), json!(task.variant));
    fields.insert("arm".into(), json!(task.arm));
    fields.insert("repeat".into(), json!(task.repeat));
    fields.insert("candidate".into(), json!(task.candidate));
    fields.insert("condition".into(), json!(task.condition));
    fields.insert("alignment".into(), json!(task.alignment));
    fields.insert("model".into(), json!(redact_error(&bundle.model)));
    fields.insert("plan_sha256".into(), json!(bundle.plan_sha256));
    fields.insert("freeze_sha256".into(), json!(bundle.freeze_sha256));
    fields.insert(
        "dataset_manifest_sha256".into(),
        json!(bundle.dataset_manifest_sha256),
    );
    fields.insert("schedule_sha256".into(), json!(bundle.schedule_sha256));
    fields.insert(
        "mid_run_correction_requested".into(),
        json!(task.mid_run_correction.is_some()),
    );
    fields
}

fn load_bundle(path: &Path) -> Result<LoadedBundle, Box<dyn std::error::Error>> {
    let bytes = fs::read(path)?;
    let bundle_sha256 = hash_bytes(&bytes);
    let bundle: Bundle = serde_json::from_slice(&bytes)?;
    if bundle.schema_version != SCHEMA_VERSION {
        return Err(format!(
            "unsupported bundle schema {}; expected {SCHEMA_VERSION}",
            bundle.schema_version
        )
        .into());
    }
    validate_identifier("study_id", &bundle.study_id)?;
    validate_model(&bundle.model)?;
    validate_sha256(&bundle.plan_sha256, "plan_sha256")?;
    validate_sha256(&bundle.freeze_sha256, "freeze_sha256")?;
    validate_sha256(&bundle.dataset_manifest_sha256, "dataset_manifest_sha256")?;
    validate_sha256(&bundle.schedule_sha256, "schedule_sha256")?;
    if bundle.tasks.is_empty() {
        return Err("bundle must contain at least one task".into());
    }
    let bundle_path = fs::canonicalize(path)?;
    let parent = bundle_path
        .parent()
        .ok_or("bundle path has no parent directory")?;
    let mut seen = BTreeSet::new();
    let mut seen_run_ids = BTreeSet::new();
    let mut seen_schedule_indices = BTreeSet::new();
    let mut seen_cells = BTreeSet::new();
    for task in &bundle.tasks {
        validate_task(task, parent)?;
        if !seen_run_ids.insert(task.run_id.clone()) {
            return Err(format!("duplicate run_id '{}'", task.run_id).into());
        }
        if !seen.insert(task.task_id.clone()) {
            return Err(format!("duplicate task_id '{}'", task.task_id).into());
        }
        if !seen_schedule_indices.insert(task.schedule_index) {
            return Err(format!("duplicate schedule_index {}", task.schedule_index).into());
        }
        let cell = (
            task.base_task_id.clone(),
            task.variant.clone(),
            task.arm.clone(),
            bundle.model.clone(),
            task.repeat,
        );
        if !seen_cells.insert(cell) {
            return Err(format!("duplicate scheduled cell for task '{}'", task.task_id).into());
        }
    }
    if seen_schedule_indices != (0..bundle.tasks.len() as u64).collect::<BTreeSet<_>>() {
        return Err("schedule_index must be the complete range 0..tasks.len()".into());
    }
    Ok(LoadedBundle {
        bundle,
        bundle_sha256,
        bundle_path,
    })
}

fn validate_task(task: &Task, bundle_parent: &Path) -> Result<(), Box<dyn std::error::Error>> {
    validate_identifier("run_id", &task.run_id)?;
    validate_identifier("task_id", &task.task_id)?;
    validate_label("repository", &task.repository)?;
    validate_identifier("base_task_id", &task.base_task_id)?;
    validate_identifier("variant", &task.variant)?;
    if task.arm != "B" && task.arm != "C" {
        return Err(format!("task '{}' arm must be B or C", task.task_id).into());
    }
    if task.alignment
        != (if task.arm == "C" {
            "enabled"
        } else {
            "disabled"
        })
    {
        return Err(format!(
            "task '{}' alignment must match arm {}",
            task.task_id, task.arm
        )
        .into());
    }
    validate_identifier("candidate", &task.candidate)?;
    validate_identifier("condition", &task.condition)?;
    if task.variant != "static" && task.variant != "correction" {
        return Err(format!(
            "task '{}' variant must be static or correction",
            task.task_id
        )
        .into());
    }
    if task.repeat > 10_000 {
        return Err(format!("task '{}' repeat is out of range", task.task_id).into());
    }
    if task.objective.trim().is_empty() || task.objective.len() > 64 * 1024 {
        return Err(format!("task '{}' has an invalid objective", task.task_id).into());
    }
    if task.maximum_seconds == 0 || task.maximum_seconds > MAX_TASK_SECONDS {
        return Err(format!("task '{}' has an invalid maximum_seconds", task.task_id).into());
    }
    if let Some(max_tokens) = task.max_tokens
        && (max_tokens == 0 || max_tokens > 131_072)
    {
        return Err(format!("task '{}' has invalid max_tokens", task.task_id).into());
    }
    match (task.variant.as_str(), task.mid_run_correction.as_deref()) {
        ("static", None) => {}
        ("static", Some(_)) => {
            return Err(format!(
                "task '{}' static variant cannot carry a mid-run correction",
                task.task_id
            )
            .into());
        }
        ("correction", Some(correction)) => validate_correction(correction, &task.task_id)?,
        ("correction", None) => {
            return Err(format!(
                "task '{}' correction variant requires mid_run_correction",
                task.task_id
            )
            .into());
        }
        _ => unreachable!("variant validated above"),
    }
    validate_sha256(&task.fixture_sha256, "fixture_sha256")?;
    let fixture = safe_relative_path(&task.fixture, "fixture")?;
    let fixture = bundle_parent.join(fixture);
    let metadata = fs::symlink_metadata(&fixture)?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(format!(
            "fixture is not a non-symlink directory: {}",
            fixture.display()
        )
        .into());
    }
    let measured = snapshot_tree(&fixture)?;
    if measured.sha256 != task.fixture_sha256.to_ascii_lowercase() {
        return Err(format!(
            "fixture hash mismatch for '{}': expected {}, got {}",
            task.task_id, task.fixture_sha256, measured.sha256
        )
        .into());
    }
    validate_path_list(&task.expected_changed_paths, "expected_changed_paths")?;
    validate_path_list(&task.forbidden_paths, "forbidden_paths")?;
    if task.expected_changed_paths.is_empty() {
        return Err(format!(
            "task '{}' must declare an expected changed path",
            task.task_id
        )
        .into());
    }
    if task.validation.program != "cargo" {
        return Err(format!("task '{}' validation program must be cargo", task.task_id).into());
    }
    if task.validation.arguments.iter().any(|argument| {
        argument.chars().any(char::is_control)
            || argument == "--target-dir"
            || argument == "--manifest-path"
            || argument.starts_with("--target-dir=")
            || argument.starts_with("--manifest-path=")
    }) {
        return Err(format!("task '{}' has unsafe validation arguments", task.task_id).into());
    }
    if task.validation.timeout_seconds == 0
        || task.validation.timeout_seconds > MAX_VALIDATION_SECONDS
    {
        return Err(format!("task '{}' has invalid validation timeout", task.task_id).into());
    }
    Ok(())
}

fn validate_correction(value: &str, task_id: &str) -> Result<(), Box<dyn std::error::Error>> {
    if value.trim().is_empty()
        || value.len() > 64 * 1024
        || value.chars().any(char::is_control)
        || value.trim_start().starts_with('/')
    {
        return Err(format!(
            "task '{task_id}' has an empty, oversized, unsafe, or command-like correction"
        )
        .into());
    }
    Ok(())
}

fn validate_model(model: &str) -> Result<(), Box<dyn std::error::Error>> {
    if model.trim().is_empty() || model.len() > 256 || model.chars().any(char::is_control) {
        return Err("bundle model is empty, too long, or contains controls".into());
    }
    Ok(())
}

fn validate_label(label: &str, value: &str) -> Result<(), Box<dyn std::error::Error>> {
    if value.trim().is_empty() || value.len() > 512 || value.chars().any(char::is_control) {
        return Err(format!("{label} is empty, too long, or contains controls").into());
    }
    Ok(())
}

fn validate_identifier(label: &str, value: &str) -> Result<(), Box<dyn std::error::Error>> {
    if value.is_empty()
        || value.len() > 256
        || !value
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'))
    {
        return Err(format!("{label} is not a safe identifier").into());
    }
    Ok(())
}

fn validate_sha256(value: &str, label: &str) -> Result<(), Box<dyn std::error::Error>> {
    if value.len() != 64
        || !value.bytes().all(|byte| byte.is_ascii_hexdigit())
        || value
            .chars()
            .any(|character| character.is_ascii_uppercase())
    {
        return Err(format!("{label} must be a 64-character lowercase SHA-256 hex digest").into());
    }
    Ok(())
}

fn validate_path_list(paths: &[String], label: &str) -> Result<(), Box<dyn std::error::Error>> {
    let mut seen = BTreeSet::new();
    for path in paths {
        let parsed = safe_relative_path(Path::new(path), label)?;
        if !seen.insert(path.replace('\\', "/")) {
            return Err(format!("duplicate path in {label}: {path}").into());
        }
        if parsed.as_os_str().is_empty() {
            return Err(format!("empty path in {label}").into());
        }
    }
    Ok(())
}

fn safe_relative_path(path: &Path, label: &str) -> Result<PathBuf, Box<dyn std::error::Error>> {
    if path.as_os_str().is_empty() || path.is_absolute() {
        return Err(format!("{label} must be a relative path").into());
    }
    for component in path.components() {
        if !matches!(component, Component::Normal(_)) {
            return Err(format!("{label} contains an unsafe path component").into());
        }
    }
    Ok(path.to_owned())
}

impl EvidenceLog {
    fn open(
        path: &Path,
        study_id: &str,
        bundle_sha256: &str,
        task_ids: BTreeSet<String>,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let exists = path.exists();
        let file = OpenOptions::new()
            .create(true)
            .read(true)
            .append(true)
            .open(path)?;
        let mut log = Self {
            file,
            study_id: study_id.to_owned(),
            bundle_sha256: bundle_sha256.to_owned(),
            sequence: 0,
            previous_hash: None,
            started: BTreeSet::new(),
            terminal: BTreeMap::new(),
            terminal_records: BTreeMap::new(),
            task_ids,
            correction_sent: BTreeSet::new(),
            correction_acknowledged: BTreeSet::new(),
        };
        if exists {
            log.read_existing(path)?;
            let terminal_ids = log.terminal.keys().cloned().collect::<BTreeSet<_>>();
            let uncertain = log
                .started
                .difference(&terminal_ids)
                .cloned()
                .collect::<Vec<_>>();
            for task_id in uncertain {
                let record = log.append_event(
                    "task_uncertain",
                    Some(&task_id),
                    json!({
                        "status": "uncertain",
                        "detail": "task_started has no terminal record; refusing replay"
                    }),
                )?;
                log.terminal.insert(task_id.clone(), "uncertain".into());
                log.terminal_records.insert(task_id, record);
            }
        } else {
            log.append_event(
                "study_started",
                None,
                json!({"task_count": log.task_ids.len()}),
            )?;
        }
        Ok(log)
    }

    fn read_existing(&mut self, path: &Path) -> Result<(), Box<dyn std::error::Error>> {
        let file = File::open(path)?;
        let reader = BufReader::new(file);
        let mut saw_study = false;
        for (index, line) in reader.lines().enumerate() {
            let line = line?;
            if line.trim().is_empty() {
                return Err(format!("blank evidence line at {}", index + 1).into());
            }
            let value: Value = serde_json::from_str(&line)?;
            self.verify_record(&value, index as u64 + 1)?;
            let event = value["event"]
                .as_str()
                .ok_or("evidence event is not a string")?;
            let task_id = value["task_id"].as_str();
            match event {
                "study_started" => {
                    if saw_study || index != 0 || task_id.is_some() {
                        return Err("invalid study_started ordering".into());
                    }
                    saw_study = true;
                }
                "task_started" => {
                    let task_id = task_id.ok_or("task_started omitted task_id")?;
                    self.require_task(task_id)?;
                    if !self.started.insert(task_id.to_owned()) {
                        return Err(format!("duplicate task_started for {task_id}").into());
                    }
                }
                "mid_run_correction_sent" => {
                    let task_id = task_id.ok_or("correction evidence omitted task_id")?;
                    self.require_task(task_id)?;
                    if !self.started.contains(task_id) || self.is_terminal(task_id) {
                        return Err(format!(
                            "correction evidence is not inside an active task for {task_id}"
                        )
                        .into());
                    }
                    if !self.correction_sent.insert(task_id.to_owned()) {
                        return Err(format!("duplicate correction sent for {task_id}").into());
                    }
                }
                // Frozen acknowledgement contract: this only means the daemon
                // durably recorded ConversationMessageAdded; it is not model acceptance.
                "requirement_revision_ack" => {
                    let task_id = task_id.ok_or("correction acknowledgement omitted task_id")?;
                    self.require_task(task_id)?;
                    if !self.started.contains(task_id) || self.is_terminal(task_id) {
                        return Err(format!(
                            "correction acknowledgement is not inside an active task for {task_id}"
                        )
                        .into());
                    }
                    if !self.correction_sent.contains(task_id) {
                        return Err(format!(
                            "correction acknowledgement before sent for {task_id}"
                        )
                        .into());
                    }
                    if !self.correction_acknowledged.insert(task_id.to_owned()) {
                        return Err(
                            format!("duplicate correction acknowledgement for {task_id}").into(),
                        );
                    }
                }
                "task_finished" | "task_error" | "task_uncertain" => {
                    let task_id = task_id.ok_or("terminal evidence omitted task_id")?;
                    self.require_task(task_id)?;
                    if !self.started.contains(task_id) {
                        return Err(
                            format!("terminal evidence before task_started for {task_id}").into(),
                        );
                    }
                    if self
                        .terminal
                        .insert(task_id.to_owned(), event.to_owned())
                        .is_some()
                    {
                        return Err(format!("duplicate terminal evidence for {task_id}").into());
                    }
                }
                other => return Err(format!("unknown evidence event '{other}'").into()),
            }
            if matches!(event, "task_finished" | "task_error" | "task_uncertain")
                && let Some(task_id) = task_id
            {
                self.terminal_records
                    .insert(task_id.to_owned(), value.clone());
            }
            self.sequence = index as u64 + 1;
            self.previous_hash = value["record_sha256"].as_str().map(str::to_owned);
        }
        if !saw_study {
            return Err("evidence log is missing study_started".into());
        }
        Ok(())
    }

    fn verify_record(
        &self,
        value: &Value,
        expected_sequence: u64,
    ) -> Result<(), Box<dyn std::error::Error>> {
        if value["schema_version"].as_u64() != Some(SCHEMA_VERSION as u64)
            || value["study_id"].as_str() != Some(self.study_id.as_str())
            || value["bundle_sha256"].as_str() != Some(self.bundle_sha256.as_str())
            || value["sequence"].as_u64() != Some(expected_sequence)
        {
            return Err("evidence identity or sequence mismatch".into());
        }
        let record_hash = value["record_sha256"]
            .as_str()
            .ok_or("evidence record omitted record_sha256")?;
        let previous = value["previous_record_sha256"].as_str().map(str::to_owned);
        if previous != self.previous_hash {
            return Err("evidence hash chain mismatch".into());
        }
        let mut unhashed = value.clone();
        unhashed
            .as_object_mut()
            .ok_or("evidence record is not an object")?
            .remove("record_sha256");
        if hash_bytes(&serde_json::to_vec(&unhashed)?) != record_hash {
            return Err("evidence record hash mismatch".into());
        }
        Ok(())
    }

    fn require_task(&self, task_id: &str) -> Result<(), Box<dyn std::error::Error>> {
        if self.task_ids.contains(task_id) {
            Ok(())
        } else {
            Err(format!("evidence references unknown task {task_id}").into())
        }
    }

    fn is_terminal(&self, task_id: &str) -> bool {
        self.terminal.contains_key(task_id)
    }

    fn correction_state(&self, task_id: &str) -> (bool, bool) {
        (
            self.correction_sent.contains(task_id),
            self.correction_acknowledged.contains(task_id),
        )
    }

    fn append_started(
        &mut self,
        task: &Task,
        model: &str,
    ) -> Result<(), Box<dyn std::error::Error>> {
        if self.is_terminal(&task.task_id) || !self.started.insert(task.task_id.clone()) {
            return Err(format!("task {} is already started", task.task_id).into());
        }
        self.append_event(
            "task_started",
            Some(&task.task_id),
            json!({
                "model": redact_error(model),
                "fixture_sha256": task.fixture_sha256.to_ascii_lowercase(),
                "maximum_seconds": task.maximum_seconds,
                "validation_timeout_seconds": task.validation.timeout_seconds,
                "mid_run_correction_requested": task.mid_run_correction.is_some(),
                "mid_run_correction_sha256": task
                    .mid_run_correction
                    .as_deref()
                    .map(normalized_correction_digest),
            }),
        )
        .map(|_| ())
    }

    fn append_correction_event(
        &mut self,
        event: &str,
        task_id: &str,
        details: Value,
    ) -> Result<(), Box<dyn std::error::Error>> {
        self.require_task(task_id)?;
        if !self.started.contains(task_id) || self.is_terminal(task_id) {
            return Err(format!("invalid correction evidence for {task_id}").into());
        }
        let mut inserted_sent = false;
        let mut inserted_acknowledged = false;
        match event {
            "mid_run_correction_sent" => {
                if !self.correction_sent.insert(task_id.to_owned()) {
                    return Err(format!("duplicate correction sent evidence for {task_id}").into());
                }
                inserted_sent = true;
            }
            // The durable event name is bound by the frozen study manifest.
            "requirement_revision_ack" => {
                if !self.correction_sent.contains(task_id) {
                    return Err(
                        format!("correction acknowledgement before sent for {task_id}").into(),
                    );
                }
                if !self.correction_acknowledged.insert(task_id.to_owned()) {
                    return Err(
                        format!("duplicate correction acknowledgement for {task_id}").into(),
                    );
                }
                inserted_acknowledged = true;
            }
            other => return Err(format!("unknown correction evidence event '{other}'").into()),
        }
        if let Err(error) = self.append_event(event, Some(task_id), details) {
            if inserted_sent {
                self.correction_sent.remove(task_id);
            }
            if inserted_acknowledged {
                self.correction_acknowledged.remove(task_id);
            }
            return Err(error);
        }
        Ok(())
    }

    fn append_terminal(
        &mut self,
        event: &str,
        task_id: &str,
        details: Value,
    ) -> Result<(), Box<dyn std::error::Error>> {
        self.require_task(task_id)?;
        if !self.started.contains(task_id) || self.is_terminal(task_id) {
            return Err(format!("invalid terminal evidence for {task_id}").into());
        }
        let status = details["status"].as_str().unwrap_or("unknown").to_owned();
        let record = self.append_event(event, Some(task_id), details)?;
        self.terminal.insert(task_id.to_owned(), status);
        self.terminal_records.insert(task_id.to_owned(), record);
        Ok(())
    }

    fn append_event(
        &mut self,
        event: &str,
        task_id: Option<&str>,
        details: Value,
    ) -> Result<Value, Box<dyn std::error::Error>> {
        let mut record = Map::new();
        record.insert("schema_version".into(), json!(SCHEMA_VERSION));
        record.insert("study_id".into(), json!(self.study_id));
        record.insert("bundle_sha256".into(), json!(self.bundle_sha256));
        record.insert("sequence".into(), json!(self.sequence + 1));
        record.insert("event".into(), json!(event));
        if let Some(task_id) = task_id {
            record.insert("task_id".into(), json!(task_id));
        }
        if let Some(fields) = details.as_object() {
            for (key, value) in fields {
                if !matches!(
                    key.as_str(),
                    "schema_version"
                        | "study_id"
                        | "bundle_sha256"
                        | "sequence"
                        | "event"
                        | "task_id"
                        | "previous_record_sha256"
                        | "record_sha256"
                ) {
                    record.insert(key.clone(), value.clone());
                }
            }
        }
        record.insert("details".into(), details);
        if let Some(previous) = &self.previous_hash {
            record.insert("previous_record_sha256".into(), json!(previous));
        }
        let unhashed = Value::Object(record.clone());
        let record_hash = hash_bytes(&serde_json::to_vec(&unhashed)?);
        record.insert("record_sha256".into(), json!(record_hash));
        let line = serde_json::to_vec(&Value::Object(record.clone()))?;
        self.file.write_all(&line)?;
        self.file.write_all(b"\n")?;
        self.file.sync_data()?;
        self.sequence += 1;
        self.previous_hash = Some(record_hash);
        Ok(Value::Object(record))
    }
}

impl ResultLog {
    fn open(path: &Path) -> Result<Self, Box<dyn std::error::Error>> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let exists = path.exists();
        let file = OpenOptions::new()
            .create(true)
            .read(true)
            .append(true)
            .open(path)?;
        let mut records = BTreeMap::new();
        if exists {
            let reader = BufReader::new(File::open(path)?);
            for (index, line) in reader.lines().enumerate() {
                let line = line?;
                if line.trim().is_empty() {
                    return Err(format!("blank result line at {}", index + 1).into());
                }
                let value: Value = serde_json::from_str(&line)?;
                let run_id = value["run_id"]
                    .as_str()
                    .ok_or("result row omitted run_id")?
                    .to_owned();
                if value["terminal_status"].as_str().is_none() {
                    return Err("result row omitted terminal_status".into());
                }
                if records.insert(run_id.clone(), value).is_some() {
                    return Err(format!("duplicate result row for {run_id}").into());
                }
            }
        }
        Ok(Self { file, records })
    }

    fn sync_from_evidence(
        &mut self,
        evidence: &EvidenceLog,
        tasks: &[Task],
        bundle: &Bundle,
    ) -> Result<(), Box<dyn std::error::Error>> {
        for task in tasks {
            if let Some(existing) = self.records.get(&task.run_id) {
                verify_result_identity(existing, task, bundle)?;
                if let Some(record) = evidence.terminal_records.get(&task.task_id)
                    && existing["source_record_sha256"] != record["record_sha256"]
                {
                    return Err(format!(
                        "result row for {} does not match immutable evidence",
                        task.run_id
                    )
                    .into());
                }
                continue;
            }
            let Some(record) = evidence.terminal_records.get(&task.task_id) else {
                continue;
            };
            let row = result_row(task, bundle, record, evidence);
            self.append(row)?;
        }
        Ok(())
    }

    fn append(&mut self, row: Value) -> Result<(), Box<dyn std::error::Error>> {
        let run_id = row["run_id"]
            .as_str()
            .ok_or("result row omitted run_id")?
            .to_owned();
        if self.records.contains_key(&run_id) {
            return Err(format!("duplicate result row for {run_id}").into());
        }
        let line = serde_json::to_vec(&row)?;
        self.file.write_all(&line)?;
        self.file.write_all(b"\n")?;
        self.file.sync_data()?;
        self.records.insert(run_id, row);
        Ok(())
    }
}

fn verify_result_identity(
    row: &Value,
    task: &Task,
    bundle: &Bundle,
) -> Result<(), Box<dyn std::error::Error>> {
    let expected = task_result_identity(task, bundle);
    for (key, value) in expected {
        if row.get(&key) != Some(&value) {
            return Err(format!("result row for {} has mismatched {key}", task.run_id).into());
        }
    }
    Ok(())
}

fn result_row(task: &Task, bundle: &Bundle, record: &Value, evidence: &EvidenceLog) -> Value {
    let mut row = task_result_identity(task, bundle);
    for key in [
        "status",
        "alignment_requested",
        "alignment_observed",
        "terminal_status",
        "oracle_status",
        "mid_run_correction_triggered",
        "mid_run_correction_acknowledged",
        "mid_run_correction_trigger_event_sha256",
        "mid_run_correction_message_event_sha256",
        "externally_correct",
        "claimed_complete",
        "session_id",
        "session_status",
        "timed_out",
        "elapsed_ms",
        "validation_passed",
        "validation_elapsed_ms",
        "baseline_tree_sha256",
        "scored_tree_sha256",
        "changed_paths",
        "missing_expected_paths",
        "forbidden_changed_paths",
        "model_calls",
        "incomplete_model_calls",
        "input_tokens",
        "output_tokens",
        "providers",
        "models",
        "session_events",
        "session_events_sha256",
        "detail",
    ] {
        if let Some(value) = record.get(key) {
            row.insert(key.into(), value.clone());
        }
    }
    let event = record["event"].as_str().unwrap_or("task_error");
    row.entry("terminal_status").or_insert_with(|| {
        json!(match event {
            "task_uncertain" => "uncertain",
            "task_error" => "error",
            _ => "valid",
        })
    });
    if row["terminal_status"] != "valid" {
        row.insert("oracle_status".into(), json!("unknown"));
        let (triggered, acknowledged) = evidence.correction_state(&task.task_id);
        row.insert("mid_run_correction_triggered".into(), json!(triggered));
        row.insert(
            "mid_run_correction_acknowledged".into(),
            json!(acknowledged),
        );
        row.insert(
            "mid_run_correction_trigger_event_sha256".into(),
            Value::Null,
        );
        row.insert(
            "mid_run_correction_message_event_sha256".into(),
            Value::Null,
        );
        row.insert("externally_correct".into(), Value::Null);
        row.insert("claimed_complete".into(), Value::Null);
    }
    if let Some(hash) = record.get("record_sha256") {
        row.insert("source_record_sha256".into(), hash.clone());
    }
    Value::Object(row)
}

async fn execute_task(
    client: &reqwest::Client,
    options: &Options,
    loaded: &LoadedBundle,
    task: &Task,
    evidence: &mut EvidenceLog,
) -> Result<TaskMeasurement, Box<dyn std::error::Error>> {
    let fixture = loaded
        .bundle_path
        .parent()
        .ok_or("bundle path has no parent")?
        .join(safe_relative_path(&task.fixture, "fixture")?);
    let baseline = snapshot_tree(&fixture)?;
    if baseline.sha256 != task.fixture_sha256.to_ascii_lowercase() {
        return Err(format!("fixture changed after bundle freeze for {}", task.task_id).into());
    }
    let repository = clone_fixture(&fixture, &task.task_id)?;
    let started = Instant::now();
    let created: Value = post_json(
        client,
        &format!("{}/v1/sessions", options.daemon.trim_end_matches('/')),
        &options.token,
        &json!({
            "objective": task.objective,
            "repository": repository,
            "task_mode": "build",
            "permission_mode": "auto",
            "model": loaded.bundle.model,
            "max_tokens": task.max_tokens,
            "alignment": task.alignment,
            "research_alignment_opt_in": task.alignment == "disabled",
        }),
    )
    .await?;
    let session_id = created["id"]
        .as_str()
        .ok_or("daemon did not return a session id")?
        .to_owned();
    let session = run_session(client, options, task, &session_id, started, evidence).await;
    let session = match session {
        Ok(session) => session,
        Err(error) => {
            let cleanup = cancel_and_confirm(
                client,
                options,
                &session_id,
                "benchmark runner failed before scoring",
            )
            .await
            .err();
            return Err(match cleanup {
                Some(cleanup) => format!(
                    "{}; cleanup failed: {}",
                    redact_error(&error.to_string()),
                    redact_error(&cleanup.to_string())
                )
                .into(),
                None => error,
            });
        }
    };

    let final_tree = if session.worktree.is_absolute() {
        session.worktree
    } else {
        return Err("daemon returned a relative worktree path".into());
    };
    let snapshot = unique_directory(&format!(
        "purrcode-e2e-snapshot-{}",
        safe_suffix(&task.task_id)
    ))?;
    copy_tree(&final_tree, &snapshot)?;
    let scored = snapshot_tree(&snapshot)?;
    let changed_paths = diff_paths(&baseline, &scored);
    let expected = task
        .expected_changed_paths
        .iter()
        .map(|path| path.replace('\\', "/"))
        .collect::<BTreeSet<_>>();
    let forbidden = task
        .forbidden_paths
        .iter()
        .map(|path| path.replace('\\', "/"))
        .collect::<BTreeSet<_>>();
    let changed_set = changed_paths.iter().cloned().collect::<BTreeSet<_>>();
    let missing_expected_paths = expected
        .difference(&changed_set)
        .cloned()
        .collect::<Vec<_>>();
    let forbidden_changed_paths = forbidden
        .intersection(&changed_set)
        .cloned()
        .collect::<Vec<_>>();

    let validation_tree = unique_directory(&format!(
        "purrcode-e2e-validation-{}",
        safe_suffix(&task.task_id)
    ))?;
    copy_tree(&snapshot, &validation_tree)?;
    let validation_target = unique_directory(&format!(
        "purrcode-e2e-target-{}",
        safe_suffix(&task.task_id)
    ))?;
    let validation_started = Instant::now();
    let validation_passed =
        run_validation(&validation_tree, &validation_target, &task.validation).await?;
    let validation_elapsed_ms = validation_started.elapsed().as_millis();
    let (model_calls, incomplete_model_calls, input_tokens, output_tokens, providers, models) =
        event_metrics(&session.events);
    let alignment_observed = observed_alignment(&session.events)?;
    let alignment_requested = task.alignment == "enabled";
    if alignment_observed != Some(alignment_requested) {
        return Err(format!(
            "requested alignment={} but durable events observed {:?}",
            task.alignment, alignment_observed
        )
        .into());
    }
    let events_bytes = serde_json::to_vec(&session.events)?;
    let externally_correct = validation_passed
        && missing_expected_paths.is_empty()
        && forbidden_changed_paths.is_empty();
    let claimed_complete = session.session_status == "completed";
    let terminal_status = if session.timed_out {
        "timeout"
    } else {
        "valid"
    };
    let oracle_status = oracle_status(session.timed_out, externally_correct);
    let status = if session.timed_out {
        "timed_out"
    } else if claimed_complete && externally_correct {
        "passed"
    } else {
        "failed"
    };
    Ok(TaskMeasurement {
        schedule_index: task.schedule_index,
        run_id: task.run_id.clone(),
        task_id: task.task_id.clone(),
        repository: task.repository.clone(),
        base_task_id: task.base_task_id.clone(),
        variant: task.variant.clone(),
        arm: task.arm.clone(),
        repeat: task.repeat,
        candidate: task.candidate.clone(),
        condition: task.condition.clone(),
        model: redact_error(&loaded.bundle.model),
        plan_sha256: loaded.bundle.plan_sha256.clone(),
        freeze_sha256: loaded.bundle.freeze_sha256.clone(),
        dataset_manifest_sha256: loaded.bundle.dataset_manifest_sha256.clone(),
        schedule_sha256: loaded.bundle.schedule_sha256.clone(),
        mid_run_correction_requested: session.correction.requested,
        mid_run_correction_triggered: session.correction.triggered,
        mid_run_correction_acknowledged: session.correction.acknowledged,
        mid_run_correction_trigger_event_sha256: session.correction.trigger_event_sha256,
        mid_run_correction_message_event_sha256: session.correction.message_event_sha256,
        alignment_requested: task.alignment.clone(),
        alignment_observed,
        terminal_status,
        oracle_status,
        externally_correct,
        claimed_complete,
        status,
        session_id: session.session_id,
        session_status: session.session_status,
        timed_out: session.timed_out,
        elapsed_ms: session.elapsed.as_millis(),
        validation_passed,
        validation_elapsed_ms,
        baseline_tree_sha256: baseline.sha256,
        scored_tree_sha256: scored.sha256,
        changed_paths,
        missing_expected_paths,
        forbidden_changed_paths,
        model_calls,
        incomplete_model_calls,
        input_tokens,
        output_tokens,
        providers,
        models,
        session_events: session.events.len() as u64,
        session_events_sha256: hash_bytes(&events_bytes),
    })
}

fn oracle_status(timed_out: bool, externally_correct: bool) -> &'static str {
    if timed_out {
        "unknown"
    } else if externally_correct {
        "correct"
    } else {
        "incorrect"
    }
}

async fn run_session(
    client: &reqwest::Client,
    options: &Options,
    task: &Task,
    session_id: &str,
    started: Instant,
    evidence: &mut EvidenceLog,
) -> Result<SessionMeasurement, Box<dyn std::error::Error>> {
    let base = options.daemon.trim_end_matches('/');
    let deadline = Duration::from_secs(task.maximum_seconds);
    let mut timed_out = false;
    let mut correction = CorrectionMeasurement {
        requested: task.mid_run_correction.is_some(),
        ..CorrectionMeasurement::default()
    };
    loop {
        let session = fetch_session(client, base, &options.token, session_id).await?;
        let status = session_status(&session);
        if correction.requested && !correction.triggered {
            let live_events: Vec<SessionEvent> = get_json(
                client,
                &format!("{base}/v1/sessions/{session_id}/events"),
                &options.token,
            )
            .await?;
            if let Some((trigger_event_sha256, affected_path_count)) =
                first_successful_mutation(&live_events)
            {
                correction = send_mid_run_correction(
                    client,
                    options,
                    task,
                    session_id,
                    CorrectionTrigger {
                        event_sha256: trigger_event_sha256,
                        affected_path_count,
                        minimum_event_index: live_events.len(),
                    },
                    evidence,
                )
                .await?;
            }
        }
        if is_terminal_status(&status) {
            break;
        }
        if started.elapsed() >= deadline {
            timed_out = true;
            request_cancel(
                client,
                options,
                session_id,
                "benchmark task deadline exceeded",
            )
            .await?;
            break;
        }
        let remaining = deadline.saturating_sub(started.elapsed());
        tokio::time::sleep(POLL_INTERVAL.min(remaining)).await;
    }
    let final_view = wait_until_stopped(client, options, session_id).await?;
    let events: Vec<SessionEvent> = get_json(
        client,
        &format!("{base}/v1/sessions/{session_id}/events"),
        &options.token,
    )
    .await?;
    if events.is_empty() {
        return Err("daemon returned no durable session events".into());
    }
    let mut state = purrcode_runtime_core::SessionState::empty(purrcode_runtime_core::SessionId(
        session_id.parse()?,
    ));
    for event in &events {
        state.reduce_event(event)?;
    }
    let worktree = state
        .worktree
        .clone()
        .or_else(|| final_view["worktree"].as_str().map(PathBuf::from))
        .ok_or("session has no settled worktree")?;
    Ok(SessionMeasurement {
        session_id: session_id.to_owned(),
        session_status: session_status(&final_view),
        timed_out,
        events,
        worktree,
        elapsed: started.elapsed(),
        correction,
    })
}

async fn send_mid_run_correction(
    client: &reqwest::Client,
    options: &Options,
    task: &Task,
    session_id: &str,
    trigger: CorrectionTrigger,
    evidence: &mut EvidenceLog,
) -> Result<CorrectionMeasurement, Box<dyn std::error::Error>> {
    let correction = task
        .mid_run_correction
        .as_deref()
        .ok_or("correction trigger fired without a correction")?;
    let content = correction.trim_end_matches([' ', '\t']);
    let message_sha256 = normalized_correction_digest(correction);
    evidence.append_correction_event(
        "mid_run_correction_sent",
        &task.task_id,
        json!({
            "trigger_event_sha256": trigger.event_sha256.clone(),
            "affected_path_count": trigger.affected_path_count,
            "message_sha256": message_sha256,
        }),
    )?;
    let http_status = post_message(client, options, session_id, content).await?;
    let message_event_sha256 = wait_for_correction_ack(
        client,
        options,
        session_id,
        trigger.minimum_event_index,
        &message_sha256,
    )
    .await?;
    evidence.append_correction_event(
        "requirement_revision_ack",
        &task.task_id,
        json!({
            "trigger_event_sha256": trigger.event_sha256.clone(),
            "message_sha256": message_sha256,
            "message_event_sha256": message_event_sha256,
            "http_status": http_status,
        }),
    )?;
    Ok(CorrectionMeasurement {
        requested: true,
        triggered: true,
        acknowledged: true,
        trigger_event_sha256: Some(trigger.event_sha256),
        message_event_sha256: Some(message_event_sha256),
    })
}

async fn post_message(
    client: &reqwest::Client,
    options: &Options,
    session_id: &str,
    content: &str,
) -> Result<u16, Box<dyn std::error::Error>> {
    let response = client
        .post(format!(
            "{}/v1/sessions/{session_id}/messages",
            options.daemon.trim_end_matches('/')
        ))
        .bearer_auth(&options.token)
        .json(&json!({"content": content}))
        .send()
        .await?;
    let status = response.status();
    if status != StatusCode::ACCEPTED {
        return Err(format!(
            "mid-run correction was not accepted (HTTP {})",
            status.as_u16()
        )
        .into());
    }
    Ok(status.as_u16())
}

async fn wait_for_correction_ack(
    client: &reqwest::Client,
    options: &Options,
    session_id: &str,
    minimum_event_index: usize,
    message_sha256: &str,
) -> Result<String, Box<dyn std::error::Error>> {
    // ConversationMessageAdded proves durable daemon recording of the user
    // message only; it does not claim that the model accepted or followed it.
    let base = options.daemon.trim_end_matches('/');
    let deadline = Instant::now() + CORRECTION_ACK_TIMEOUT;
    loop {
        let events: Vec<SessionEvent> = get_json(
            client,
            &format!("{base}/v1/sessions/{session_id}/events"),
            &options.token,
        )
        .await?;
        if let Some(event) = events.iter().skip(minimum_event_index).find(|event| {
            matches!(
                event,
                SessionEvent::ConversationMessageAdded { message }
                    if message.role == "user"
                        && normalized_correction_digest(&message.content) == message_sha256
            )
        }) {
            return Ok(hash_bytes(&serde_json::to_vec(event)?));
        }
        if Instant::now() >= deadline {
            return Err("daemon accepted correction but did not durably acknowledge it".into());
        }
        tokio::time::sleep(POLL_INTERVAL.min(deadline.saturating_duration_since(Instant::now())))
            .await;
    }
}

fn first_successful_mutation(events: &[SessionEvent]) -> Option<(String, usize)> {
    for (index, event) in events.iter().enumerate() {
        let SessionEvent::ExecutionFinished {
            action_id,
            exit_code: Some(0),
            affected_paths,
            ..
        } = event
        else {
            continue;
        };
        if affected_paths.is_empty() {
            continue;
        }
        let evidence = events[..index].iter().rev().find_map(|candidate| {
            if let SessionEvent::ToolEvidenceRecorded { evidence } = candidate
                && evidence.action_id == *action_id
            {
                Some(evidence)
            } else {
                None
            }
        });
        let is_mutation = evidence.is_none_or(|evidence| {
            evidence_is_mutation(
                evidence.tool_id.as_str(),
                &evidence.effective_filesystem_scope,
            )
        });
        if is_mutation {
            return Some((
                hash_bytes(&serde_json::to_vec(event).ok()?),
                affected_paths.len(),
            ));
        }
    }
    None
}

fn evidence_is_mutation(tool_id: &str, scope: &FilesystemScope) -> bool {
    matches!(scope, FilesystemScope::Worktree { .. })
        || matches!(tool_id, "native:write_file" | "native:delete_file")
}

fn normalized_correction_digest(value: &str) -> String {
    hash_bytes(value.trim_end_matches([' ', '\t']).as_bytes())
}

fn is_terminal_status(status: &str) -> bool {
    matches!(
        status,
        "completed"
            | "failed"
            | "cancelled"
            | "paused"
            | "awaiting_review"
            | "awaiting_approval"
            | "ready"
    )
}

async fn post_json<T: DeserializeOwned>(
    client: &reqwest::Client,
    url: &str,
    token: &str,
    body: &Value,
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
) -> Result<Value, Box<dyn std::error::Error>> {
    get_json(client, &format!("{base}/v1/sessions/{session_id}"), token).await
}

fn session_status(session: &Value) -> String {
    session["status_code"]
        .as_str()
        .or_else(|| session["status"].as_str())
        .unwrap_or_default()
        .to_ascii_lowercase()
}

async fn request_cancel(
    client: &reqwest::Client,
    options: &Options,
    session_id: &str,
    reason: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let response = client
        .post(format!(
            "{}/v1/sessions/{session_id}/cancel",
            options.daemon.trim_end_matches('/')
        ))
        .bearer_auth(&options.token)
        .json(&json!({"reason": reason}))
        .send()
        .await?;
    if response.status() == StatusCode::CONFLICT {
        return Ok(());
    }
    response.error_for_status()?;
    Ok(())
}

async fn cancel_and_confirm(
    client: &reqwest::Client,
    options: &Options,
    session_id: &str,
    reason: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let cancel = request_cancel(client, options, session_id, reason).await;
    let drained = wait_until_stopped(client, options, session_id).await;
    match (cancel, drained) {
        (Ok(()), Ok(_)) => Ok(()),
        (Err(cancel), Ok(_)) => Err(format!("cancel request failed: {cancel}").into()),
        (Ok(()), Err(drain)) => Err(format!("session did not drain: {drain}").into()),
        (Err(cancel), Err(drain)) => {
            Err(format!("cancel failed: {cancel}; drain failed: {drain}").into())
        }
    }
}

async fn wait_until_stopped(
    client: &reqwest::Client,
    options: &Options,
    session_id: &str,
) -> Result<Value, Box<dyn std::error::Error>> {
    let base = options.daemon.trim_end_matches('/');
    let deadline = Instant::now() + STOP_CONFIRM_TIMEOUT;
    loop {
        let session = fetch_session(client, base, &options.token, session_id).await?;
        let supervisor: Value = get_json(
            client,
            &format!("{base}/v1/supervisor/{session_id}"),
            &options.token,
        )
        .await?;
        if session_stopped(&session) && supervisor_workers_stopped(&supervisor) {
            return Ok(session);
        }
        if Instant::now() >= deadline {
            return Err("could not confirm settled session and drained workers".into());
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

fn session_stopped(session: &Value) -> bool {
    session["lease_active"].as_bool() == Some(false) && is_terminal_status(&session_status(session))
}

fn supervisor_workers_stopped(supervisor: &Value) -> bool {
    supervisor
        .get("workers")
        .and_then(Value::as_array)
        .is_some_and(|workers| {
            workers
                .iter()
                .all(|worker| worker["status"].as_str().is_some_and(worker_status_stopped))
        })
}

fn worker_status_stopped(status: &str) -> bool {
    let status = status.to_ascii_lowercase();
    matches!(status.as_str(), "completed" | "interrupted" | "failed")
        || status.starts_with("failed:")
        || status.starts_with("skipped dependency:")
}

fn clone_fixture(source: &Path, suffix: &str) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let destination =
        unique_directory(&format!("purrcode-e2e-repository-{}", safe_suffix(suffix)))?;
    copy_tree(source, &destination)?;
    for arguments in [
        vec!["init", "-q"],
        vec!["add", "-A"],
        vec![
            "-c",
            "user.name=PurrCode Benchmark",
            "-c",
            "user.email=benchmark@local.invalid",
            "commit",
            "-q",
            "-m",
            "fixture",
        ],
    ] {
        let status = std::process::Command::new("git")
            .args(arguments)
            .current_dir(&destination)
            .env_clear()
            .envs(clean_environment())
            .env("GIT_TERMINAL_PROMPT", "0")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()?;
        if !status.success() {
            return Err("could not initialise isolated benchmark repository".into());
        }
    }
    Ok(destination)
}

fn unique_directory(prefix: &str) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let root = std::env::temp_dir();
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    for attempt in 0..100_u32 {
        let candidate = root.join(format!("{prefix}-{}-{stamp}-{attempt}", std::process::id()));
        match fs::create_dir(&candidate) {
            Ok(()) => return Ok(candidate),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error.into()),
        }
    }
    Err(format!("could not allocate a temporary directory for {prefix}").into())
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

fn copy_tree(source: &Path, destination: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let mut budget = CopyBudget::default();
    copy_tree_with_budget(source, destination, &mut budget)
}

#[derive(Default)]
struct CopyBudget {
    files: usize,
    bytes: u64,
}

fn copy_tree_with_budget(
    source: &Path,
    destination: &Path,
    budget: &mut CopyBudget,
) -> Result<(), Box<dyn std::error::Error>> {
    let root_type = fs::symlink_metadata(source)?.file_type();
    if root_type.is_symlink() || !root_type.is_dir() {
        return Err(format!("refusing to copy non-directory tree {}", source.display()).into());
    }
    fs::create_dir_all(destination)?;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let name = entry.file_name();
        if matches!(name.to_str(), Some(".git") | Some("target")) {
            continue;
        }
        let file_type = entry.file_type()?;
        if file_type.is_symlink() {
            return Err(format!("refusing to copy symlink {}", entry.path().display()).into());
        }
        let from = entry.path();
        let to = destination.join(name);
        if file_type.is_dir() {
            copy_tree_with_budget(&from, &to, budget)?;
        } else if file_type.is_file() {
            let size = entry.metadata()?.len();
            budget.files = budget.files.saturating_add(1);
            budget.bytes = budget.bytes.saturating_add(size);
            if budget.files > MAX_COPY_FILES || budget.bytes > MAX_COPY_BYTES {
                return Err("benchmark tree exceeds copy budget".into());
            }
            fs::copy(&from, &to)?;
        } else {
            return Err(format!("unsupported filesystem entry {}", from.display()).into());
        }
    }
    Ok(())
}

fn snapshot_tree(root: &Path) -> Result<TreeSnapshot, Box<dyn std::error::Error>> {
    let mut files = BTreeMap::new();
    let mut budget = CopyBudget::default();
    collect_files(root, root, &mut files, &mut budget)?;
    let mut digest = Sha256::new();
    for (path, file_hash) in &files {
        digest.update(path.as_bytes());
        digest.update([0]);
        digest.update(file_hash.as_bytes());
        digest.update([0]);
    }
    Ok(TreeSnapshot {
        files,
        sha256: format!("{:x}", digest.finalize()),
    })
}

fn collect_files(
    root: &Path,
    current: &Path,
    files: &mut BTreeMap<String, String>,
    budget: &mut CopyBudget,
) -> Result<(), Box<dyn std::error::Error>> {
    let metadata = fs::symlink_metadata(current)?;
    if metadata.file_type().is_symlink() {
        return Err(format!("refusing to hash symlink {}", current.display()).into());
    }
    if !metadata.is_dir() {
        return Err(format!("tree root is not a directory: {}", current.display()).into());
    }
    for entry in fs::read_dir(current)? {
        let entry = entry?;
        let name = entry.file_name();
        if matches!(name.to_str(), Some(".git") | Some("target")) {
            continue;
        }
        let file_type = entry.file_type()?;
        if file_type.is_symlink() {
            return Err(format!("refusing to hash symlink {}", entry.path().display()).into());
        }
        let path = entry.path();
        if file_type.is_dir() {
            collect_files(root, &path, files, budget)?;
        } else if file_type.is_file() {
            let bytes = fs::read(&path)?;
            budget.files = budget.files.saturating_add(1);
            budget.bytes = budget.bytes.saturating_add(bytes.len() as u64);
            if budget.files > MAX_COPY_FILES || budget.bytes > MAX_COPY_BYTES {
                return Err("benchmark tree exceeds snapshot budget".into());
            }
            let relative = path
                .strip_prefix(root)
                .map_err(|_| "tree path escaped root")?
                .to_string_lossy()
                .replace('\\', "/");
            files.insert(relative, hash_bytes(&bytes));
        } else {
            return Err(format!("unsupported filesystem entry {}", path.display()).into());
        }
    }
    Ok(())
}

fn diff_paths(before: &TreeSnapshot, after: &TreeSnapshot) -> Vec<String> {
    before
        .files
        .keys()
        .chain(after.files.keys())
        .cloned()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .filter(|path| before.files.get(path) != after.files.get(path))
        .collect()
}

async fn run_validation(
    worktree: &Path,
    target_dir: &Path,
    validation: &Validation,
) -> Result<bool, Box<dyn std::error::Error>> {
    let mut command = tokio::process::Command::new(&validation.program);
    command
        .args(&validation.arguments)
        .current_dir(worktree)
        .env_clear()
        .envs(clean_environment())
        .env("CARGO_TARGET_DIR", target_dir)
        .env("CARGO_NET_OFFLINE", "true")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    #[cfg(unix)]
    command.process_group(0);
    let Ok(mut child) = command.spawn() else {
        return Ok(false);
    };
    match tokio::time::timeout(
        Duration::from_secs(validation.timeout_seconds),
        child.wait(),
    )
    .await
    {
        Ok(Ok(status)) => Ok(status.success()),
        Ok(Err(_)) => Ok(false),
        Err(_) => {
            #[cfg(unix)]
            let pid = child.id();
            #[cfg(unix)]
            let signalled = terminate_process_group(&mut child);
            #[cfg(not(unix))]
            let signalled = false;
            if !signalled {
                let _ = child.kill().await;
            }
            match tokio::time::timeout(Duration::from_secs(1), child.wait()).await {
                Ok(Ok(_)) => {
                    #[cfg(unix)]
                    if let Some(pid) = pid {
                        wait_for_process_group_exit(pid).await?;
                    }
                    Ok(false)
                }
                Ok(Err(error)) => {
                    Err(format!("validation wait failed after timeout: {error}").into())
                }
                Err(_) => Err("validation process did not stop after timeout".into()),
            }
        }
    }
}

#[cfg(unix)]
fn terminate_process_group(child: &mut tokio::process::Child) -> bool {
    let Some(pid) = child.id() else {
        return false;
    };
    let group = format!("-{pid}");
    let term = std::process::Command::new("kill")
        .args(["-TERM", &group])
        .env_clear()
        .envs(clean_environment())
        .status()
        .is_ok_and(|status| status.success());
    let kill = std::process::Command::new("kill")
        .args(["-KILL", &group])
        .env_clear()
        .envs(clean_environment())
        .status()
        .is_ok_and(|status| status.success());
    term || kill
}

#[cfg(unix)]
async fn wait_for_process_group_exit(pid: u32) -> Result<(), Box<dyn std::error::Error>> {
    let group = format!("-{pid}");
    let deadline = Instant::now() + Duration::from_secs(1);
    loop {
        let status = std::process::Command::new("kill")
            .args(["-0", &group])
            .env_clear()
            .envs(clean_environment())
            .status()?;
        if !status.success() {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err("validation descendants did not stop".into());
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

fn event_metrics(events: &[SessionEvent]) -> (u64, u64, u64, u64, Vec<String>, Vec<String>) {
    let mut started = 0_u64;
    let mut finished = 0_u64;
    let mut input_tokens = 0_u64;
    let mut output_tokens = 0_u64;
    let mut providers = BTreeSet::new();
    let mut models = BTreeSet::new();
    for event in events {
        match event {
            SessionEvent::ModelRequestStarted {
                provider, model, ..
            } => {
                started += 1;
                providers.insert(redact_error(provider));
                models.insert(redact_error(model));
            }
            SessionEvent::ModelRequestFinished {
                input_tokens: input,
                output_tokens: output,
                ..
            } => {
                finished += 1;
                input_tokens = input_tokens.saturating_add(input.unwrap_or(0));
                output_tokens = output_tokens.saturating_add(output.unwrap_or(0));
            }
            _ => {}
        }
    }
    (
        started.max(finished),
        started.saturating_sub(finished),
        input_tokens,
        output_tokens,
        providers.into_iter().collect(),
        models.into_iter().collect(),
    )
}

fn observed_alignment(events: &[SessionEvent]) -> Result<Option<bool>, Box<dyn std::error::Error>> {
    let durable_mode = events.iter().rev().find_map(|event| match event {
        SessionEvent::SessionControlsUpdated { controls } => Some(matches!(
            controls.alignment,
            purrcode_runtime_core::adaptation::AlignmentMode::Enabled
        )),
        _ => None,
    });
    let enabled_evidence = events.iter().any(|event| {
        matches!(
            event,
            SessionEvent::ExpectationContractCreated { .. }
                | SessionEvent::ExpectationContractRevised { .. }
                | SessionEvent::DeliveryGateEvaluated { .. }
        ) || matches!(
            event,
            SessionEvent::ModelRequestStarted { role, .. }
                if role == "planner" || role == "reviewer" || role == "alignment_reviewer"
        )
    });
    if durable_mode == Some(false) && enabled_evidence {
        return Err("durable alignment=disabled conflicts with alignment events".into());
    }
    Ok(durable_mode.or_else(|| enabled_evidence.then_some(true)))
}

fn hash_bytes(bytes: &[u8]) -> String {
    let mut digest = Sha256::new();
    digest.update(bytes);
    format!("{:x}", digest.finalize())
}

fn redact_error(value: &str) -> String {
    let mut output = String::with_capacity(value.len().min(512));
    let mut token = String::new();
    for character in value.chars() {
        if character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.') {
            token.push(character);
        } else {
            append_redacted_token(&mut output, &token);
            token.clear();
            output.push(character);
        }
        if output.len() >= 512 {
            break;
        }
    }
    append_redacted_token(&mut output, &token);
    output.truncate(512);
    output
}

fn append_redacted_token(output: &mut String, token: &str) {
    if token.starts_with("sk-")
        || token.starts_with("sk_")
        || token.starts_with("dsk-")
        || token.len() > 40 && token.chars().all(|character| character.is_ascii_hexdigit())
    {
        output.push_str("[REDACTED]");
    } else {
        output.push_str(token);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn fixture_hash(path: &Path) -> String {
        snapshot_tree(path).expect("fixture snapshot").sha256
    }

    fn task(path: &Path) -> Task {
        Task {
            schedule_index: 0,
            run_id: "task-1-B-0".into(),
            task_id: "task-1".into(),
            repository: "fixture-repo".into(),
            base_task_id: "task-1".into(),
            variant: "correction".into(),
            arm: "B".into(),
            repeat: 0,
            candidate: "gold".into(),
            condition: "revision_rubric_json".into(),
            alignment: "disabled".into(),
            objective: "make the test pass".into(),
            fixture: PathBuf::from("fixture"),
            fixture_sha256: fixture_hash(path),
            expected_changed_paths: vec!["src/lib.rs".into()],
            forbidden_paths: vec![],
            max_tokens: Some(1024),
            mid_run_correction: Some("tighten the implementation".into()),
            maximum_seconds: 30,
            validation: Validation {
                program: "cargo".into(),
                arguments: vec!["test".into(), "-q".into()],
                timeout_seconds: 30,
            },
        }
    }

    #[test]
    fn snapshot_digest_and_changed_paths_are_order_independent() {
        let directory = tempdir().expect("tempdir");
        fs::create_dir(directory.path().join("src")).expect("src");
        fs::write(directory.path().join("src/lib.rs"), "one").expect("source");
        let before = snapshot_tree(directory.path()).expect("before");
        fs::write(directory.path().join("src/lib.rs"), "two").expect("changed");
        fs::write(directory.path().join("new.txt"), "new").expect("new");
        let after = snapshot_tree(directory.path()).expect("after");
        assert_ne!(before.sha256, after.sha256);
        assert_eq!(diff_paths(&before, &after), vec!["new.txt", "src/lib.rs"]);
    }

    #[test]
    fn bundle_validation_rejects_changed_fixture_and_unsafe_paths() {
        let directory = tempdir().expect("tempdir");
        let fixture = directory.path().join("fixture");
        fs::create_dir(&fixture).expect("fixture");
        fs::write(fixture.join("README"), "fixture").expect("fixture file");
        let mut case = task(&fixture);
        case.fixture = PathBuf::from("../fixture");
        assert!(validate_task(&case, directory.path()).is_err());
        case.fixture = PathBuf::from("fixture");
        fs::write(fixture.join("README"), "modified").expect("modify");
        assert!(validate_task(&case, directory.path()).is_err());
        assert!(validate_sha256(&"A".repeat(64), "plan_sha256").is_err());
    }

    #[test]
    fn variant_requires_matching_correction_payload() {
        let directory = tempdir().expect("tempdir");
        let fixture = directory.path().join("fixture");
        fs::create_dir(&fixture).expect("fixture");
        fs::write(fixture.join("README"), "fixture").expect("fixture file");
        let mut case = task(&fixture);
        case.variant = "static".into();
        case.mid_run_correction = None;
        assert!(validate_task(&case, directory.path()).is_ok());
        case.mid_run_correction = Some("unexpected correction".into());
        assert!(validate_task(&case, directory.path()).is_err());
        case.variant = "correction".into();
        assert!(validate_task(&case, directory.path()).is_ok());
        case.mid_run_correction = None;
        assert!(validate_task(&case, directory.path()).is_err());
    }

    #[test]
    fn evidence_log_hashes_records_and_marks_started_tasks_uncertain() {
        let directory = tempdir().expect("tempdir");
        let fixture = directory.path().join("fixture");
        fs::create_dir(&fixture).expect("fixture");
        fs::write(fixture.join("README"), "fixture").expect("fixture file");
        let mut case = task(&fixture);
        case.mid_run_correction = Some("tighten the implementation".into());
        let path = directory.path().join("events.jsonl");
        let mut log = EvidenceLog::open(
            &path,
            "study",
            &"a".repeat(64),
            [case.task_id.clone()].into_iter().collect(),
        )
        .expect("open");
        log.append_started(&case, "small-model").expect("start");
        log.append_correction_event(
            "mid_run_correction_sent",
            &case.task_id,
            json!({"message_sha256": "b".repeat(64)}),
        )
        .expect("correction sent");
        log.append_correction_event(
            "requirement_revision_ack",
            &case.task_id,
            json!({"message_event_sha256": "c".repeat(64)}),
        )
        .expect("correction acknowledged");
        assert!(
            log.append_correction_event(
                "mid_run_correction_sent",
                &case.task_id,
                json!({"message_sha256": "d".repeat(64)}),
            )
            .is_err()
        );
        drop(log);
        let resumed = EvidenceLog::open(
            &path,
            "study",
            &"a".repeat(64),
            [case.task_id.clone()].into_iter().collect(),
        )
        .expect("resume");
        assert_eq!(
            resumed.terminal.get("task-1"),
            Some(&"uncertain".to_owned())
        );
        let lines = fs::read_to_string(path).expect("events");
        assert_eq!(lines.lines().count(), 5);
        assert!(lines.lines().all(|line| line.contains("record_sha256")));
        let results_path = directory.path().join("results.jsonl");
        let mut results = ResultLog::open(&results_path).expect("results");
        let bundle = Bundle {
            schema_version: SCHEMA_VERSION,
            study_id: "study".into(),
            model: "small-model".into(),
            plan_sha256: "b".repeat(64),
            freeze_sha256: "c".repeat(64),
            dataset_manifest_sha256: "d".repeat(64),
            schedule_sha256: "e".repeat(64),
            tasks: Vec::new(),
        };
        results
            .sync_from_evidence(&resumed, &[case], &bundle)
            .expect("sync results");
        let row: Value = serde_json::from_str(
            fs::read_to_string(results_path)
                .expect("result line")
                .lines()
                .next()
                .expect("one result"),
        )
        .expect("valid result");
        assert_eq!(row["run_id"], "task-1-B-0");
        assert_eq!(row["arm"], "B");
        assert_eq!(row["terminal_status"], "uncertain");
        assert_eq!(row["oracle_status"], "unknown");
        assert_eq!(row["study_id"], "study");
        assert_eq!(row["plan_sha256"], "b".repeat(64));
        assert_eq!(row["freeze_sha256"], "c".repeat(64));
        assert_eq!(row["dataset_manifest_sha256"], "d".repeat(64));
        assert_eq!(row["schedule_sha256"], "e".repeat(64));
        assert_eq!(row["mid_run_correction_requested"], true);
        assert_eq!(row["mid_run_correction_triggered"], true);
        assert_eq!(row["mid_run_correction_acknowledged"], true);
        assert!(row["externally_correct"].is_null());
    }

    #[test]
    fn evidence_log_rejects_tampering() {
        let directory = tempdir().expect("tempdir");
        let path = directory.path().join("events.jsonl");
        let task_ids: BTreeSet<String> = ["task-1".to_owned()].into_iter().collect();
        let _log =
            EvidenceLog::open(&path, "study", &"a".repeat(64), task_ids.clone()).expect("open");
        let mut contents = fs::read_to_string(&path).expect("read");
        contents = contents.replace("study", "other");
        fs::write(&path, contents).expect("tamper");
        assert!(EvidenceLog::open(&path, "study", &"a".repeat(64), task_ids).is_err());
    }

    #[test]
    fn validation_environment_excludes_provider_credentials() {
        let environment = clean_environment();
        assert!(environment.iter().all(|(key, _)| {
            !matches!(
                key.as_str(),
                "OPENAI_API_KEY"
                    | "ANTHROPIC_API_KEY"
                    | "DEEPSEEK_API_KEY"
                    | "NVIDIA_API_KEY"
                    | "OLLAMA_API_KEY"
            )
        }));
    }

    #[test]
    fn redaction_handles_provider_shaped_tokens() {
        let value = redact_error("request sk-abcdef1234567890 and dsk-abcdef1234567890 failed");
        assert!(!value.contains("sk-abcdef"));
        assert!(!value.contains("dsk-abcdef"));
        assert!(value.contains("[REDACTED]"));
    }

    #[test]
    fn oracle_mapping_distinguishes_correct_incorrect_and_unsettled() {
        assert_eq!(oracle_status(false, true), "correct");
        assert_eq!(oracle_status(false, false), "incorrect");
        assert_eq!(oracle_status(true, true), "unknown");
    }

    #[test]
    fn correction_trigger_uses_first_successful_nonempty_mutation() {
        let failed_event = SessionEvent::ExecutionFinished {
            action_id: purrcode_runtime_core::ActionId::new(),
            exit_code: Some(1),
            truncated: false,
            sandbox_level: None,
            sandbox_backend: None,
            affected_paths: vec![PathBuf::from("tests/fixture.rs")],
        };
        assert!(first_successful_mutation(&[failed_event]).is_none());
        let empty_event = SessionEvent::ExecutionFinished {
            action_id: purrcode_runtime_core::ActionId::new(),
            exit_code: Some(0),
            truncated: false,
            sandbox_level: None,
            sandbox_backend: None,
            affected_paths: vec![],
        };
        assert!(first_successful_mutation(&[empty_event]).is_none());
        let test_path_event = SessionEvent::ExecutionFinished {
            action_id: purrcode_runtime_core::ActionId::new(),
            exit_code: Some(0),
            truncated: false,
            sandbox_level: None,
            sandbox_backend: None,
            affected_paths: vec![PathBuf::from("tests/fixture.rs")],
        };
        assert!(first_successful_mutation(&[test_path_event]).is_some());
    }

    #[test]
    fn read_evidence_is_not_a_mutation_trigger() {
        assert!(!evidence_is_mutation(
            "native:read_file",
            &FilesystemScope::WorktreeRead
        ));
        assert!(evidence_is_mutation(
            "native:write_file",
            &FilesystemScope::WorktreeRead
        ));
        assert!(evidence_is_mutation(
            "native:repository_grep",
            &FilesystemScope::Worktree {
                write_globs: vec!["**".into()],
                maximum_changed_files: 1,
            }
        ));
    }

    #[test]
    fn alignment_observation_uses_durable_controls_and_rejects_conflicts() {
        let mut controls = purrcode_runtime_core::adaptation::SessionControls::default();
        controls.alignment = purrcode_runtime_core::adaptation::AlignmentMode::Disabled;
        let disabled = SessionEvent::SessionControlsUpdated { controls };
        assert_eq!(observed_alignment(&[disabled]).unwrap(), Some(false));

        let mut controls = purrcode_runtime_core::adaptation::SessionControls::default();
        controls.alignment = purrcode_runtime_core::adaptation::AlignmentMode::Enabled;
        let enabled = SessionEvent::SessionControlsUpdated { controls };
        assert_eq!(observed_alignment(&[enabled]).unwrap(), Some(true));

        let alignment_evidence = SessionEvent::ModelRequestStarted {
            role: "alignment_reviewer".into(),
            provider: "mock".into(),
            model: "mock/model".into(),
        };
        let mut controls = purrcode_runtime_core::adaptation::SessionControls::default();
        controls.alignment = purrcode_runtime_core::adaptation::AlignmentMode::Disabled;
        assert!(
            observed_alignment(&[
                SessionEvent::SessionControlsUpdated { controls },
                alignment_evidence,
            ])
            .is_err()
        );
    }
}
