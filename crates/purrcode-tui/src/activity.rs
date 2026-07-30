//! Derivation of user-facing progress from durable session events.
//!
//! The workbench must answer eight questions without the user reading event
//! JSON: what task is active, what PurrCode is doing, what it is waiting for,
//! whether a decision needs attention, which files may be affected, what
//! validation concluded, whether the session is safe, and what to do next.
//!
//! Everything here is a pure function over the durable event list, so all of it
//! is covered by unit tests rather than by rendering assertions. No presentation
//! code re-derives any of these facts, and nothing invents a second source of
//! truth for changed files: repository effects come from the repository engine's
//! own porcelain evidence.

use serde_json::Value;

// ── Activity rail ────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ActivityState {
    /// Finished successfully.
    Done,
    /// Running right now.
    Active,
    /// Expected but not started.
    Pending,
    /// Needs the user.
    Attention,
    /// Finished unsuccessfully.
    Failed,
}

impl ActivityState {
    /// Glyph plus an always-present word, so status never depends on colour or
    /// on a symbol alone.
    pub const fn glyph(self, unicode: bool) -> &'static str {
        match (self, unicode) {
            (Self::Done, true) => "✓",
            (Self::Done, false) => "[x]",
            (Self::Active, true) => "●",
            (Self::Active, false) => "[>]",
            (Self::Pending, true) => "○",
            (Self::Pending, false) => "[ ]",
            (Self::Attention, true) => "!",
            (Self::Attention, false) => "[!]",
            (Self::Failed, true) => "✗",
            (Self::Failed, false) => "[f]",
        }
    }

    pub const fn word(self) -> &'static str {
        match self {
            Self::Done => "done",
            Self::Active => "running",
            Self::Pending => "pending",
            Self::Attention => "needs you",
            Self::Failed => "failed",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ActivityEntry {
    pub state: ActivityState,
    pub label: String,
    pub detail: Option<String>,
    /// Index of the durable event that produced this entry, for the inspector.
    pub event_index: Option<usize>,
}

impl ActivityEntry {
    fn new(state: ActivityState, label: impl Into<String>, event_index: Option<usize>) -> Self {
        Self {
            state,
            label: label.into(),
            detail: None,
            event_index,
        }
    }

    fn detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }
}

// ── What PurrCode is waiting for ─────────────────────────────────

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum WaitingOn {
    #[default]
    Nothing,
    Model,
    Tool,
    Validation,
    User,
}

impl WaitingOn {
    /// One short sentence naming what the interface is blocked on.
    pub const fn describe(self) -> &'static str {
        match self {
            Self::Nothing => "Ready for your next task",
            Self::Model => "Waiting for the model",
            Self::Tool => "Waiting for a tool to finish",
            Self::Validation => "Waiting for validation",
            Self::User => "Waiting for you to decide",
        }
    }
}

// ── Session state ────────────────────────────────────────────────

/// How safe the session is to act on. `Uncertain` must never be presented as a
/// success, which is why it is a distinct variant rather than a flag on
/// `Completed`.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum SessionState {
    #[default]
    Safe,
    Paused,
    Completed,
    Uncertain,
    Recoverable,
    Failed,
}

impl SessionState {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Safe => "safe",
            Self::Paused => "paused",
            Self::Completed => "completed",
            Self::Uncertain => "uncertain",
            Self::Recoverable => "recovery required",
            Self::Failed => "failed",
        }
    }

    /// True when the state must not be rendered as a successful outcome.
    pub const fn needs_attention(self) -> bool {
        matches!(
            self,
            Self::Uncertain | Self::Recoverable | Self::Failed | Self::Paused
        )
    }
}

// ── Repository effects ───────────────────────────────────────────

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EffectStatus {
    Added,
    Modified,
    Deleted,
    Renamed,
    Untracked,
    Unknown,
}

impl EffectStatus {
    /// Single-letter indicator; always paired with [`Self::word`] in the UI so
    /// the meaning never depends on colour.
    pub const fn letter(self) -> &'static str {
        match self {
            Self::Added => "A",
            Self::Modified => "M",
            Self::Deleted => "D",
            Self::Renamed => "R",
            Self::Untracked => "?",
            Self::Unknown => "-",
        }
    }

    pub const fn word(self) -> &'static str {
        match self {
            Self::Added => "added",
            Self::Modified => "modified",
            Self::Deleted => "deleted",
            Self::Renamed => "renamed",
            Self::Untracked => "new",
            Self::Unknown => "unknown",
        }
    }

    fn from_porcelain(code: &str) -> Self {
        let code = code.trim();
        if code.contains('R') {
            return Self::Renamed;
        }
        if code.contains('D') {
            return Self::Deleted;
        }
        if code == "??" {
            return Self::Untracked;
        }
        if code.contains('A') {
            return Self::Added;
        }
        if code.contains('M') {
            return Self::Modified;
        }
        Self::Unknown
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FileEffect {
    pub path: String,
    pub status: EffectStatus,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RepositoryEffects {
    pub files: Vec<FileEffect>,
    /// Paths a proposed-but-unapproved action would touch. These are shown as
    /// "may be affected", never as recorded changes.
    pub proposed_paths: Vec<String>,
}

impl RepositoryEffects {
    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }

    pub fn summary(&self) -> String {
        self.summary_with(" · ")
    }

    pub fn summary_with(&self, separator: &str) -> String {
        if self.files.is_empty() {
            return "no recorded changes".into();
        }
        let added = self.count(EffectStatus::Added) + self.count(EffectStatus::Untracked);
        let modified = self.count(EffectStatus::Modified) + self.count(EffectStatus::Renamed);
        let deleted = self.count(EffectStatus::Deleted);
        let mut parts = Vec::new();
        if added > 0 {
            parts.push(format!("{added} added"));
        }
        if modified > 0 {
            parts.push(format!("{modified} modified"));
        }
        if deleted > 0 {
            parts.push(format!("{deleted} deleted"));
        }
        if parts.is_empty() {
            parts.push(format!("{} changed", self.files.len()));
        }
        parts.join(separator)
    }

    fn count(&self, status: EffectStatus) -> usize {
        self.files
            .iter()
            .filter(|file| file.status == status)
            .count()
    }
}

/// Parse `git status --porcelain` output produced by the repository engine.
///
/// This is deliberately the only place changed-file status is interpreted: the
/// review screen and the activity rail both read the result, so they cannot
/// disagree about what changed.
pub fn effects_from_porcelain(porcelain: &str) -> Vec<FileEffect> {
    porcelain
        .lines()
        .filter(|line| line.len() > 3)
        .filter_map(|line| {
            let (code, remainder) = line.split_at(2);
            let path = remainder.trim();
            // Renames are reported as `old -> new`; the new path is what the
            // user reviews.
            let path = path.rsplit(" -> ").next().unwrap_or(path);
            (!path.is_empty()).then(|| FileEffect {
                path: path.to_owned(),
                status: EffectStatus::from_porcelain(code),
            })
        })
        .collect()
}

// ── Validation ───────────────────────────────────────────────────

/// Validation outcome. `Unavailable`, `TimedOut` and `Uncertain` are distinct
/// from `Failed` because the correct user action differs in each case, and none
/// of them may be shown as a pass.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ValidationState {
    #[default]
    NotRun,
    Running,
    Passed,
    Failed,
    Unavailable,
    NotDetected,
    SkippedByConfiguration,
    TimedOut,
    Uncertain,
}

impl ValidationState {
    fn from_status(status: &str) -> Self {
        match status {
            "passed" => Self::Passed,
            "failed" => Self::Failed,
            "unavailable" => Self::Unavailable,
            "not_detected" => Self::NotDetected,
            "skipped_by_configuration" => Self::SkippedByConfiguration,
            "timed_out" => Self::TimedOut,
            "uncertain" => Self::Uncertain,
            _ => Self::Uncertain,
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::NotRun => "not run",
            Self::Running => "running",
            Self::Passed => "passed",
            Self::Failed => "failed",
            Self::Unavailable => "unavailable",
            Self::NotDetected => "no validation detected",
            Self::SkippedByConfiguration => "skipped by configuration",
            Self::TimedOut => "timed out",
            Self::Uncertain => "uncertain",
        }
    }

    /// True only when validation positively passed. Everything else — including
    /// unavailable and timed out — is not a pass.
    pub const fn is_pass(self) -> bool {
        matches!(self, Self::Passed)
    }

    /// True when the user must look at this before trusting the result.
    pub const fn needs_attention(self) -> bool {
        matches!(
            self,
            Self::Failed | Self::Unavailable | Self::NotDetected | Self::TimedOut | Self::Uncertain
        )
    }

    /// What the user should do next about this validation state.
    pub const fn next_action(self) -> &'static str {
        match self {
            Self::NotRun => "Validation has not run yet",
            Self::Running => "Validation is running",
            Self::Passed => "Review the diff, then apply or roll back",
            Self::Failed => "Read the failing checks, then correct or roll back",
            Self::Unavailable => "No validator ran; verify manually before applying",
            Self::NotDetected => "No validator was detected; verify manually",
            Self::SkippedByConfiguration => "Validation was skipped by configuration",
            Self::TimedOut => "Validation did not finish; rerun it or verify manually",
            Self::Uncertain => "The result is uncertain; verify manually before applying",
        }
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ValidationSummary {
    pub state: ValidationState,
    pub evidence: String,
    /// Number of recorded validation events, so repeats are visible.
    pub records: usize,
}

// ── Combined progress ────────────────────────────────────────────

#[derive(Clone, Debug, Default)]
pub struct WorkbenchProgress {
    pub objective: String,
    pub activity: Vec<ActivityEntry>,
    pub waiting_on: WaitingOn,
    pub session_state: SessionState,
    pub effects: RepositoryEffects,
    pub validation: ValidationSummary,
}

impl WorkbenchProgress {
    /// The single line that answers "what is PurrCode doing?".
    pub fn current_activity(&self) -> String {
        if let Some(entry) = self
            .activity
            .iter()
            .rev()
            .find(|entry| entry.state == ActivityState::Attention)
        {
            return entry.label.clone();
        }
        self.activity
            .iter()
            .rev()
            .find(|entry| entry.state == ActivityState::Active)
            .map(|entry| entry.label.clone())
            .unwrap_or_else(|| self.waiting_on.describe().to_owned())
    }
}

/// Derive everything the workbench shows from the durable event list.
pub fn derive(events: &[Value]) -> WorkbenchProgress {
    let mut progress = WorkbenchProgress::default();
    let mut inspected = 0usize;
    let mut inspected_index = None;
    let mut editing: Option<(String, usize)> = None;
    let mut edited: Vec<String> = Vec::new();
    let mut awaiting_approval: Option<usize> = None;
    let mut validation_started: Option<usize> = None;
    let mut model_pending: Option<usize> = None;
    let mut tool_pending: Option<usize> = None;
    let mut terminal = None;

    for (index, event) in events.iter().enumerate() {
        let name = event.get("event").and_then(Value::as_str).unwrap_or("");
        let data = event.get("data").unwrap_or(&Value::Null);
        let text = |key: &str| data.get(key).and_then(Value::as_str).unwrap_or("");

        match name {
            "session_created" => {
                progress.objective = text("objective").to_owned();
                progress.activity.push(ActivityEntry::new(
                    ActivityState::Done,
                    "Task received",
                    Some(index),
                ));
            }
            "context_indexed" => {
                let files = data
                    .get("files")
                    .and_then(Value::as_u64)
                    .unwrap_or_default();
                let symbols = data
                    .get("symbols")
                    .and_then(Value::as_u64)
                    .unwrap_or_default();
                progress.activity.push(
                    ActivityEntry::new(ActivityState::Done, "Context prepared", Some(index))
                        .detail(format!("{files} files · {symbols} symbols")),
                );
            }
            "context_compacted" => {
                progress.activity.push(
                    ActivityEntry::new(ActivityState::Done, "Context compacted", Some(index))
                        .detail(text("summary").to_owned()),
                );
            }
            "plan_created" | "plan_revised" => {
                let steps = data
                    .get("steps")
                    .and_then(Value::as_array)
                    .map_or(0, Vec::len);
                progress.activity.push(
                    ActivityEntry::new(ActivityState::Done, "Plan created", Some(index))
                        .detail(format!("{steps} step(s)")),
                );
            }
            "model_request_started" | "provider_request_started" => {
                model_pending = Some(index);
            }
            "model_request_finished" => model_pending = None,
            "action_proposed" => {
                let action = data.get("action").unwrap_or(&Value::Null);
                let kind = action.get("type").and_then(Value::as_str).unwrap_or("");
                let path = action.get("path").and_then(Value::as_str);
                match kind {
                    "write_file" | "delete_file" => {
                        if let Some(path) = path {
                            progress.effects.proposed_paths.push(path.to_owned());
                            editing = Some((path.to_owned(), index));
                        }
                    }
                    _ => {
                        inspected = inspected.saturating_add(1);
                        inspected_index = Some(index);
                    }
                }
            }
            "judgment_recorded" => {
                let decision = data
                    .pointer("/decision/decision")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                if decision == "require_approval" {
                    awaiting_approval = Some(index);
                }
            }
            "approval_recorded" | "authorization_persisted" => awaiting_approval = None,
            "approval_rejected" => {
                awaiting_approval = None;
                editing = None;
                progress.effects.proposed_paths.clear();
                progress.activity.push(
                    ActivityEntry::new(ActivityState::Done, "Action rejected", Some(index))
                        .detail("nothing was executed".to_owned()),
                );
            }
            "execution_started" => {
                tool_pending = Some(index);
            }
            "execution_finished" => {
                tool_pending = None;
                let failed = data
                    .get("exit_code")
                    .and_then(Value::as_i64)
                    .is_some_and(|code| code != 0);
                if let Some((path, _)) = editing.take() {
                    if !failed {
                        edited.push(path);
                    }
                }
                if failed {
                    progress.activity.push(
                        ActivityEntry::new(ActivityState::Failed, "Tool failed", Some(index))
                            .detail(format!(
                                "exit {}",
                                data.get("exit_code")
                                    .and_then(Value::as_i64)
                                    .unwrap_or_default()
                            )),
                    );
                }
            }
            "validation_started" => validation_started = Some(index),
            "validation_recorded" => {
                validation_started = None;
                let state = ValidationState::from_status(text("status"));
                progress.validation = ValidationSummary {
                    state,
                    evidence: text("evidence").to_owned(),
                    records: progress.validation.records.saturating_add(1),
                };
                let activity_state = if state.is_pass() {
                    ActivityState::Done
                } else if state.needs_attention() {
                    ActivityState::Failed
                } else {
                    ActivityState::Done
                };
                progress.activity.push(
                    ActivityEntry::new(
                        activity_state,
                        format!("Validation {}", state.label()),
                        Some(index),
                    )
                    .detail(text("evidence").to_owned()),
                );
            }
            "checkpoint_created" => {
                progress.activity.push(
                    ActivityEntry::new(ActivityState::Done, "Checkpoint created", Some(index))
                        .detail(text("label").to_owned()),
                );
            }
            "recovery_required" => {
                terminal = Some(SessionState::Recoverable);
                progress.activity.push(
                    ActivityEntry::new(ActivityState::Attention, "Recovery required", Some(index))
                        .detail(text("reason").to_owned()),
                );
            }
            "session_paused" | "outcome_review_required" => {
                terminal = Some(SessionState::Paused);
                progress.activity.push(
                    ActivityEntry::new(
                        ActivityState::Attention,
                        "Outcome review required",
                        Some(index),
                    )
                    .detail(text("reason").to_owned()),
                );
            }
            "session_failed" => {
                terminal = Some(SessionState::Failed);
                progress.activity.push(
                    ActivityEntry::new(ActivityState::Failed, "Session failed", Some(index))
                        .detail(text("reason").to_owned()),
                );
            }
            "session_cancelled" => {
                terminal = Some(SessionState::Recoverable);
                progress.activity.push(
                    ActivityEntry::new(ActivityState::Failed, "Session cancelled", Some(index))
                        .detail(text("reason").to_owned()),
                );
            }
            "session_completed" => {
                terminal = Some(SessionState::Completed);
                progress.activity.push(ActivityEntry::new(
                    ActivityState::Done,
                    "Completed",
                    Some(index),
                ));
            }
            _ => {}
        }
    }

    // Aggregate repeated inspections into one compact entry rather than one
    // line per read, and place it before any later stage.
    if inspected > 0 {
        let entry = ActivityEntry::new(
            ActivityState::Done,
            format!("{inspected} file(s) inspected"),
            inspected_index,
        );
        let position = progress
            .activity
            .iter()
            .position(|existing| existing.label.starts_with("Validation"))
            .unwrap_or(progress.activity.len());
        progress.activity.insert(position, entry);
    }
    for path in &edited {
        progress.activity.push(ActivityEntry::new(
            ActivityState::Done,
            format!("Edited {path}"),
            None,
        ));
    }
    if let Some((path, index)) = &editing {
        progress.activity.push(ActivityEntry::new(
            ActivityState::Active,
            format!("Editing {path}"),
            Some(*index),
        ));
    }
    if let Some(index) = awaiting_approval {
        progress.activity.push(
            ActivityEntry::new(
                ActivityState::Attention,
                "Awaiting your approval",
                Some(index),
            )
            .detail("nothing runs until you decide".to_owned()),
        );
    }
    if validation_started.is_some() {
        progress.validation.state = ValidationState::Running;
        progress.activity.push(ActivityEntry::new(
            ActivityState::Active,
            "Validation running",
            validation_started,
        ));
    } else if progress.validation.state == ValidationState::NotRun && !edited.is_empty() {
        progress.activity.push(ActivityEntry::new(
            ActivityState::Pending,
            "Validation pending",
            None,
        ));
    }

    progress.waiting_on = if awaiting_approval.is_some()
        || matches!(
            terminal,
            Some(SessionState::Paused | SessionState::Recoverable)
        ) {
        WaitingOn::User
    } else if validation_started.is_some() {
        WaitingOn::Validation
    } else if tool_pending.is_some() {
        WaitingOn::Tool
    } else if model_pending.is_some() {
        WaitingOn::Model
    } else {
        WaitingOn::Nothing
    };

    // A session that recorded a non-passing validation is not a plain success,
    // even when the runtime reported completion.
    progress.session_state = match terminal {
        Some(SessionState::Completed) if progress.validation.state.needs_attention() => {
            SessionState::Uncertain
        }
        Some(state) => state,
        None => SessionState::Safe,
    };

    // Keep the rail honest too: a green "Completed" beside an uncertain outcome
    // is exactly the mismatch that makes a user trust an unverified result.
    if progress.session_state == SessionState::Uncertain {
        if let Some(entry) = progress
            .activity
            .iter_mut()
            .find(|entry| entry.label == "Completed")
        {
            entry.state = ActivityState::Attention;
            entry.label = "Completed with unverified results".to_owned();
            entry.detail = Some(format!("validation {}", progress.validation.state.label()));
        }
    }

    progress
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn events(names: Vec<Value>) -> Vec<Value> {
        names
    }

    #[test]
    fn activity_rail_is_compact_and_ordered() {
        let progress = derive(&events(vec![
            json!({"event":"session_created","data":{"objective":"Add a test"}}),
            json!({"event":"context_indexed","data":{"files":42,"symbols":300,"sensitive_files":0}}),
            json!({"event":"plan_created","data":{"steps":["a","b","c","d"]}}),
            json!({"event":"action_proposed","data":{"action":{"type":"read_file","path":"a.rs"}}}),
            json!({"event":"action_proposed","data":{"action":{"type":"read_file","path":"b.rs"}}}),
            json!({"event":"action_proposed","data":{"action":{"type":"read_file","path":"c.rs"}}}),
            json!({"event":"action_proposed","data":{"action":{"type":"write_file","path":"src/runtime.rs"}}}),
            json!({"event":"execution_started","data":{"action_id":"x"}}),
        ]));
        let labels: Vec<&str> = progress
            .activity
            .iter()
            .map(|entry| entry.label.as_str())
            .collect();
        assert_eq!(
            labels,
            vec![
                "Task received",
                "Context prepared",
                "Plan created",
                "3 file(s) inspected",
                "Editing src/runtime.rs",
            ],
            "three reads must collapse into one entry"
        );
        assert_eq!(progress.activity[4].state, ActivityState::Active);
        assert_eq!(progress.objective, "Add a test");
    }

    #[test]
    fn waiting_on_names_the_actual_blocker() {
        let model = derive(&[json!({"event":"model_request_started","data":{"role":"coder"}})]);
        assert_eq!(model.waiting_on, WaitingOn::Model);

        let tool = derive(&[
            json!({"event":"model_request_finished","data":{}}),
            json!({"event":"execution_started","data":{"action_id":"a"}}),
        ]);
        assert_eq!(tool.waiting_on, WaitingOn::Tool);

        let validating = derive(&[json!({"event":"validation_started","data":{}})]);
        assert_eq!(validating.waiting_on, WaitingOn::Validation);

        let user = derive(&[
            json!({"event":"action_proposed","data":{"action_id":"a","action":{"type":"write_file","path":"x.rs"}}}),
            json!({"event":"judgment_recorded","data":{"action_id":"a","decision":{"decision":"require_approval"}}}),
        ]);
        assert_eq!(user.waiting_on, WaitingOn::User);
        assert_eq!(user.current_activity(), "Awaiting your approval");

        let idle = derive(&[json!({"event":"session_completed","data":{}})]);
        assert_eq!(idle.waiting_on, WaitingOn::Nothing);
    }

    #[test]
    fn approval_clears_after_a_decision_and_rejection_records_no_execution() {
        let rejected = derive(&[
            json!({"event":"action_proposed","data":{"action_id":"a","action":{"type":"write_file","path":"x.rs"}}}),
            json!({"event":"judgment_recorded","data":{"action_id":"a","decision":{"decision":"require_approval"}}}),
            json!({"event":"approval_rejected","data":{"action_id":"a","reason":"not wanted"}}),
        ]);
        assert_eq!(rejected.waiting_on, WaitingOn::Nothing);
        assert!(rejected.effects.proposed_paths.is_empty());
        assert!(rejected
            .activity
            .iter()
            .any(|entry| entry.label == "Action rejected"));
    }

    #[test]
    fn unavailable_and_timed_out_validation_are_never_a_pass() {
        for (status, expected) in [
            ("passed", ValidationState::Passed),
            ("failed", ValidationState::Failed),
            ("unavailable", ValidationState::Unavailable),
            ("timed_out", ValidationState::TimedOut),
            ("uncertain", ValidationState::Uncertain),
            ("not_detected", ValidationState::NotDetected),
            (
                "skipped_by_configuration",
                ValidationState::SkippedByConfiguration,
            ),
        ] {
            let progress = derive(&[
                json!({"event":"validation_recorded","data":{"status":status,"evidence":"ev"}}),
            ]);
            assert_eq!(progress.validation.state, expected, "status {status}");
            assert_eq!(
                progress.validation.state.is_pass(),
                status == "passed",
                "only `passed` may read as a pass; {status} must not"
            );
        }
    }

    #[test]
    fn completion_with_failing_validation_is_uncertain_not_success() {
        let progress = derive(&[
            json!({"event":"validation_recorded","data":{"status":"failed","evidence":"3 checks failed"}}),
            json!({"event":"session_completed","data":{}}),
        ]);
        assert_eq!(progress.session_state, SessionState::Uncertain);
        assert!(progress.session_state.needs_attention());
    }

    #[test]
    fn completion_with_unavailable_validation_is_uncertain() {
        let progress = derive(&[
            json!({"event":"validation_recorded","data":{"status":"unavailable","evidence":"no validator"}}),
            json!({"event":"session_completed","data":{}}),
        ]);
        assert_eq!(progress.session_state, SessionState::Uncertain);
    }

    #[test]
    fn an_uncertain_outcome_is_not_marked_complete_in_the_activity_rail() {
        let progress = derive(&[
            json!({"event":"validation_recorded","data":{"status":"unavailable","evidence":"none"}}),
            json!({"event":"session_completed","data":{}}),
        ]);
        assert_eq!(progress.session_state, SessionState::Uncertain);
        let completion = progress
            .activity
            .iter()
            .find(|entry| entry.label.starts_with("Completed"))
            .expect("a completion entry must exist");
        assert_eq!(
            completion.state,
            ActivityState::Attention,
            "a green completion beside an uncertain outcome misleads the user"
        );
        assert_eq!(completion.label, "Completed with unverified results");
        assert!(!progress
            .activity
            .iter()
            .any(|entry| entry.label == "Completed" && entry.state == ActivityState::Done));
    }

    #[test]
    fn clean_completion_is_completed() {
        let progress = derive(&[
            json!({"event":"validation_recorded","data":{"status":"passed","evidence":"ok"}}),
            json!({"event":"session_completed","data":{}}),
        ]);
        assert_eq!(progress.session_state, SessionState::Completed);
        assert!(!progress.session_state.needs_attention());
        assert!(progress
            .activity
            .iter()
            .any(|entry| entry.label == "Completed" && entry.state == ActivityState::Done));
    }

    #[test]
    fn proposed_write_is_listed_as_may_be_affected_not_as_a_change() {
        let progress = derive(&[
            json!({"event":"action_proposed","data":{"action_id":"a","action":{"type":"write_file","path":"src/lib.rs"}}}),
        ]);
        assert_eq!(progress.effects.proposed_paths, vec!["src/lib.rs"]);
        assert!(
            progress.effects.is_empty(),
            "a proposal is not a recorded repository change"
        );
    }

    #[test]
    fn failed_tool_execution_is_visible_and_not_counted_as_an_edit() {
        let progress = derive(&[
            json!({"event":"action_proposed","data":{"action_id":"a","action":{"type":"write_file","path":"x.rs"}}}),
            json!({"event":"execution_started","data":{"action_id":"a"}}),
            json!({"event":"execution_finished","data":{"action_id":"a","exit_code":1}}),
        ]);
        assert!(progress
            .activity
            .iter()
            .any(|entry| entry.state == ActivityState::Failed && entry.label == "Tool failed"));
        assert!(!progress
            .activity
            .iter()
            .any(|entry| entry.label.starts_with("Edited")));
    }

    #[test]
    fn successful_edit_records_pending_validation() {
        let progress = derive(&[
            json!({"event":"action_proposed","data":{"action_id":"a","action":{"type":"write_file","path":"x.rs"}}}),
            json!({"event":"execution_started","data":{"action_id":"a"}}),
            json!({"event":"execution_finished","data":{"action_id":"a","exit_code":0}}),
        ]);
        let labels: Vec<&str> = progress
            .activity
            .iter()
            .map(|entry| entry.label.as_str())
            .collect();
        assert!(labels.contains(&"Edited x.rs"), "{labels:?}");
        assert!(labels.contains(&"Validation pending"), "{labels:?}");
    }

    #[test]
    fn porcelain_status_maps_to_reviewable_file_effects() {
        let effects = effects_from_porcelain(
            " M src/lib.rs\nA  src/new.rs\n D src/gone.rs\n?? notes.txt\nR  old.rs -> new.rs\n",
        );
        assert_eq!(
            effects,
            vec![
                FileEffect {
                    path: "src/lib.rs".into(),
                    status: EffectStatus::Modified
                },
                FileEffect {
                    path: "src/new.rs".into(),
                    status: EffectStatus::Added
                },
                FileEffect {
                    path: "src/gone.rs".into(),
                    status: EffectStatus::Deleted
                },
                FileEffect {
                    path: "notes.txt".into(),
                    status: EffectStatus::Untracked
                },
                FileEffect {
                    path: "new.rs".into(),
                    status: EffectStatus::Renamed
                },
            ]
        );
    }

    #[test]
    fn effect_summary_counts_by_status() {
        let effects = RepositoryEffects {
            files: effects_from_porcelain(" M a.rs\nA  b.rs\n D c.rs\n"),
            proposed_paths: Vec::new(),
        };
        assert_eq!(effects.summary(), "1 added · 1 modified · 1 deleted");
        assert_eq!(
            RepositoryEffects::default().summary(),
            "no recorded changes"
        );
    }

    #[test]
    fn every_status_has_a_word_so_colour_is_never_the_only_signal() {
        for status in [
            EffectStatus::Added,
            EffectStatus::Modified,
            EffectStatus::Deleted,
            EffectStatus::Renamed,
            EffectStatus::Untracked,
            EffectStatus::Unknown,
        ] {
            assert!(!status.word().is_empty());
            assert!(!status.letter().is_empty());
        }
        for state in [
            ActivityState::Done,
            ActivityState::Active,
            ActivityState::Pending,
            ActivityState::Attention,
            ActivityState::Failed,
        ] {
            assert!(!state.word().is_empty());
            assert!(!state.glyph(true).is_empty());
            assert!(state.glyph(false).is_ascii(), "ASCII fallback required");
        }
    }

    #[test]
    fn derivation_of_ten_thousand_events_stays_responsive() {
        let events: Vec<Value> = (0..10_000)
            .map(|index| {
                json!({"event":"validation_recorded","data":{"status":"passed","evidence":format!("check {index}")}})
            })
            .collect();
        let started = std::time::Instant::now();
        let progress = derive(&events);
        assert_eq!(progress.validation.records, 10_000);
        assert!(
            started.elapsed() < std::time::Duration::from_millis(250),
            "derivation must stay interactive at 10,000 events"
        );
    }

    #[test]
    fn every_waiting_state_and_session_state_has_a_description() {
        for waiting in [
            WaitingOn::Nothing,
            WaitingOn::Model,
            WaitingOn::Tool,
            WaitingOn::Validation,
            WaitingOn::User,
        ] {
            assert!(!waiting.describe().is_empty());
        }
        for state in [
            SessionState::Safe,
            SessionState::Paused,
            SessionState::Completed,
            SessionState::Uncertain,
            SessionState::Recoverable,
            SessionState::Failed,
        ] {
            assert!(!state.label().is_empty());
        }
        for state in [
            ValidationState::NotRun,
            ValidationState::Running,
            ValidationState::Passed,
            ValidationState::Failed,
            ValidationState::Unavailable,
            ValidationState::NotDetected,
            ValidationState::SkippedByConfiguration,
            ValidationState::TimedOut,
            ValidationState::Uncertain,
        ] {
            assert!(!state.label().is_empty());
            assert!(
                !state.next_action().is_empty(),
                "every validation state must tell the user what to do next"
            );
        }
    }
}
