//! View models: the daemon's JSON, narrowed to what the screen draws.
//!
//! The IDE deliberately parses defensively rather than deriving `Deserialize`
//! on the daemon's exact shapes. A field the daemon has not shipped yet must
//! degrade to "not available" — never to a blank window or a panic — because
//! the same binary has to talk to a daemon that may be a version behind.
//!
//! Nothing here re-derives product vocabulary from raw events (PRD §31): the
//! canonical state comes from the daemon, and [`ProductState`] is only used to
//! *interpret* the label the daemon already chose.

use chrono::{DateTime, Utc};
use purrcode_runtime_core::{ProductState, ProductStateView, SpanId, TurnId};
use serde_json::Value;
use std::collections::BTreeMap;
use uuid::Uuid;

use crate::daemon::{PanelAvailability, PanelKind, PanelResult};

/// One row in the session list.
#[derive(Clone, Debug)]
pub struct SessionRow {
    pub id: String,
    /// The folder this session belongs to. Kept so a client can refuse to show
    /// another folder's work even if it talks to a daemon that does not filter.
    pub repository: Option<String>,
    pub title: String,
    pub state: ProductState,
    pub relative_time: String,
    pub needs_attention: bool,
    pub group: String,
    pub unread: bool,
    /// Workspace metadata from `session_meta`. Archived sessions are hidden
    /// behind a disclosure rather than deleted; pinned ones sort to the top.
    pub archived: bool,
    pub pinned: bool,
    /// The session this one was forked from, so a fork is visibly a branch
    /// rather than an unrelated session that appeared out of nowhere.
    pub parent_id: Option<String>,
}

/// One conversation turn.
///
/// `turn_id` correlates this message with the `run_until_pause` iteration
/// that produced it (PRD v1.1 §6.3), replacing the position-based "last user
/// message" guess `work_log_anchor` used to make. `None` means the daemon
/// genuinely did not stamp one — a user-typed message created outside
/// `run_until_pause`, or a message recorded before turn ids existed — never a
/// synthesized id that would coincidentally fail to match anything.
/// `span_id`/`parent_span_id` are reserved for the nested-work-unit identity
/// later phases add (e.g. a Scout exploration step); Phase 1 does not
/// populate them.
#[derive(Clone, Debug, Default)]
pub struct Message {
    /// The daemon's message id. This is what `POST /v1/sessions/{id}/fork`
    /// anchors on, so forking from a message is exact rather than positional.
    /// Empty for a message recorded before ids were persisted, which is why
    /// the fork affordance is hidden rather than offered-and-broken.
    pub id: String,
    pub role: String,
    pub content: String,
    pub timestamp: String,
    pub turn_id: Option<TurnId>,
    pub span_id: Option<SpanId>,
    pub parent_span_id: Option<SpanId>,
}

impl Message {
    pub fn is_user(&self) -> bool {
        self.role.eq_ignore_ascii_case("user")
    }
}

/// One line of semantic progress.
///
/// Carries the same `turn_id`/`span_id`/`parent_span_id` triple as [`Message`]
/// so the Work Log can be anchored to the request that produced it by exact
/// identity rather than by scanning for the most recent user message. `None`
/// when the item is derived from an aggregation with no single owning turn.
#[derive(Clone, Debug, Default)]
pub struct ActivityLine {
    pub label: String,
    pub status: String,
    pub summary: Option<String>,
    pub turn_id: Option<TurnId>,
    pub span_id: Option<SpanId>,
    pub parent_span_id: Option<SpanId>,
}

impl ActivityLine {
    /// The marker name for this status, for `icons::step_marker`.
    ///
    /// A name rather than a character: egui's bundled fonts have no tick or
    /// cross, so a text glyph renders as an empty tofu box — which reads as an
    /// unticked checkbox and makes a passed step look pending. The status word
    /// always travels with the marker, so nothing means anything by shape
    /// alone (PRD §27).
    pub fn marker(&self) -> &'static str {
        match self.status.as_str() {
            "done" => "done",
            "running" => "running",
            "failed" => "failed",
            "blocked" => "blocked",
            _ => "pending",
        }
    }
}

/// Fold a raw activity stream into a semantic checklist.
///
/// A long build emits the same two lines dozens of times — "Ran a command",
/// "Validation failed" — and rendering each one turns the conversation into a
/// runtime log, which PRD §14 caps at a quarter of the area and §8 says must
/// not displace the task. Consecutive repeats collapse into one line carrying a
/// count, so the shape of the work stays readable and nothing is invented.
pub fn condense(lines: &[ActivityLine]) -> Vec<(ActivityLine, usize)> {
    let mut folded: Vec<(ActivityLine, usize)> = Vec::new();
    for line in lines {
        // Context indexing can be observed more than once while a session is
        // restored. Keep one entry for each distinct result (for example
        // `0/0` and then `12/34`) and let the latest identical observation win
        // even when another activity line appeared between them. A changed
        // result remains visible, so this is de-duplication rather than
        // hiding progress.
        if let Some(key) = context_index_key(line) {
            if let Some(index) = folded.iter().position(|(previous, _)| {
                context_index_key(previous).as_deref() == Some(key.as_str())
            }) {
                folded.remove(index);
            }
            folded.push((line.clone(), 1));
            continue;
        }
        match folded.last_mut() {
            Some((previous, count))
                if previous.label == line.label && previous.status == line.status =>
            {
                *count += 1;
                // Keep the most recent detail: the latest failure is the one
                // worth reading.
                previous.summary = line.summary.clone().or_else(|| previous.summary.clone());
            }
            _ => folded.push((line.clone(), 1)),
        }
    }
    folded
}

fn context_index_key(line: &ActivityLine) -> Option<String> {
    let label = line.label.trim().to_ascii_lowercase();
    if !label.contains("indexed") {
        return None;
    }
    Some(format!(
        "{label}\u{1f}{}\u{1f}{}",
        line.status.trim().to_ascii_lowercase(),
        line.summary.as_deref().unwrap_or_default().trim()
    ))
}

/// A first-class output: a plan, a set of changes, a validation result.
#[derive(Clone, Debug)]
pub struct Artifact {
    pub kind: String,
    pub title: String,
    pub summary: String,
    pub steps: Vec<ActivityLine>,
    pub files: Vec<String>,
    pub actions: Vec<ArtifactAction>,
}

#[derive(Clone, Debug)]
pub struct ArtifactAction {
    pub label: String,
    pub command: String,
}

/// A durable task graph row, kept separate from the legacy flat plan strings.
#[derive(Clone, Debug, Default)]
pub struct WorkTask {
    pub id: String,
    pub objective: String,
    pub status: String,
    pub current_activity: Option<String>,
    pub evidence_summary: Option<String>,
}

/// Requirement/evidence coverage shown alongside validation. A non-passing
/// status is preserved verbatim as a product status rather than inferred from
/// the number of tests that happened to run.
#[derive(Clone, Debug, Default)]
pub struct CoverageRow {
    pub id: String,
    pub statement: String,
    pub status: String,
    pub summary: Option<String>,
}

/// Which set of changes the panel is showing.
///
/// "The diff" is three questions once the agent works in its own worktree
/// (PRD §23), and collapsing them hides committed work.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ChangeScope {
    #[default]
    Agent,
    WorkingTree,
    Staged,
}

impl ChangeScope {
    pub const ALL: &'static [Self] = &[Self::Agent, Self::WorkingTree, Self::Staged];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Agent => "Agent changes",
            Self::WorkingTree => "Working tree",
            Self::Staged => "Staged",
        }
    }

    pub const fn slug(self) -> &'static str {
        match self {
            Self::Agent => "agent",
            Self::WorkingTree => "working_tree",
            Self::Staged => "staged",
        }
    }
}

/// One changed file, with what happened to it.
#[derive(Clone, Debug)]
pub struct ChangedFile {
    pub path: String,
    /// `M`, `A`, `D`, `R`. A deleted file shown as modified sends the reviewer
    /// looking for a file that is not there.
    pub status: char,
    pub additions: Option<usize>,
    pub deletions: Option<usize>,
}

impl ChangedFile {
    pub fn status_word(&self) -> &'static str {
        match self.status {
            'A' => "added",
            'D' => "deleted",
            'R' => "renamed",
            'C' => "copied",
            _ => "modified",
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct Changes {
    pub scope: ChangeScope,
    pub files_changed: usize,
    pub additions: usize,
    pub deletions: usize,
    pub files: Vec<String>,
    pub entries: Vec<ChangedFile>,
    /// `false` when there is no worktree yet, so the UI says "no changes yet"
    /// instead of "0 files changed" as though it had checked.
    pub available: bool,
}

impl Changes {
    /// The one-line summary. Says "no changes" rather than "0 files" when the
    /// answer is genuinely zero, and stays silent when nothing was checked.
    pub fn summary(&self) -> String {
        if !self.available {
            return "Not checked yet".to_owned();
        }
        if self.files_changed == 0 {
            return format!("No {}", self.scope.label().to_lowercase());
        }
        format!(
            "{} file{} · +{} −{}",
            self.files_changed,
            if self.files_changed == 1 { "" } else { "s" },
            self.additions,
            self.deletions
        )
    }
}

#[derive(Clone, Debug, Default)]
pub struct Validation {
    pub stages: Vec<ValidationStage>,
    pub complete: bool,
    pub headline: String,
}

impl Validation {
    pub fn passed(&self) -> usize {
        self.stages.iter().filter(|stage| stage.passed()).count()
    }

    pub fn failed(&self) -> usize {
        self.stages
            .iter()
            .filter(|stage| stage.outcome == "failed")
            .count()
    }

    /// The aggregated line PRD §29 asks for, instead of a wall of repeated
    /// "Validation failed" events. `None` when nothing has run — which is not
    /// the same as everything having passed.
    pub fn aggregate(&self) -> Option<String> {
        if self.stages.is_empty() {
            return None;
        }
        let passed = self.passed();
        let total = self.stages.len();
        Some(if passed == total {
            format!("All {total} checks passed")
        } else {
            format!("{passed} / {total} checks passed")
        })
    }

    /// The headline verb. PRD §36: `Paused` describes runtime mechanics; a
    /// user needs to know something wants their attention.
    pub fn needs_attention(&self) -> bool {
        self.failed() > 0
    }
}

/// Where a problem came from, so the panel groups by something actionable.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum ProblemCategory {
    Build,
    Tests,
    Lint,
    TypeChecking,
    Environment,
    Dependencies,
    Runtime,
}

impl ProblemCategory {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Build => "Build",
            Self::Tests => "Tests",
            Self::Lint => "Lint",
            Self::TypeChecking => "Type checking",
            Self::Environment => "Environment",
            Self::Dependencies => "Dependencies",
            Self::Runtime => "Runtime",
        }
    }

    /// Classify from the stage name and its detail.
    ///
    /// A missing executable is an *environment* problem even when a test stage
    /// reported it: "install ruff" and "fix the failing test" are different
    /// actions, and filing the first under Tests sends the user to the wrong
    /// place (PRD §28).
    pub fn classify(stage: &str, detail: Option<&str>) -> Self {
        let haystack = format!(
            "{} {}",
            stage.to_ascii_lowercase(),
            detail.unwrap_or_default().to_ascii_lowercase()
        );
        // A tool that is missing, or a shell environment the process could not
        // read, is an environment problem whichever stage tripped over it:
        // "install ruff" and "fix the failing test" are different actions, and
        // filing the first under Tests sends the user to the wrong place.
        if haystack.contains("not installed")
            || haystack.contains("command not found")
            || haystack.contains("execvp")
            || haystack.contains("no such file or directory")
            || haystack.contains("$home")
            || haystack.contains("home directory")
            || haystack.contains("uv_os_homedir")
        {
            return Self::Environment;
        }
        if haystack.contains("could not resolve")
            || haystack.contains("unresolved import")
            || haystack.contains("dependency")
            || haystack.contains("lockfile")
        {
            return Self::Dependencies;
        }
        if haystack.contains("type") && haystack.contains("check") {
            return Self::TypeChecking;
        }
        if haystack.contains("lint") || haystack.contains("clippy") || haystack.contains("format") {
            return Self::Lint;
        }
        if haystack.contains("test") {
            return Self::Tests;
        }
        if haystack.contains("build") || haystack.contains("compil") {
            return Self::Build;
        }
        Self::Runtime
    }
}

/// One thing the user could act on.
#[derive(Clone, Debug)]
pub struct Problem {
    pub category: ProblemCategory,
    /// A sentence, not a stack trace.
    pub summary: String,
    /// The raw runtime message, shown under "Technical details".
    pub detail: Option<String>,
    /// `path:line` when the message named one, so the panel can open it.
    pub location: Option<(String, usize)>,
}

/// Turn validation stages into problems.
///
/// A stage that did not pass is not automatically a problem: `unavailable` and
/// `skipped` mean the check never ran, and listing those as failures would tell
/// the user to fix something that was never broken (PRD §24.3).
pub fn problems_from(validation: &Validation) -> Vec<Problem> {
    let mut problems: Vec<Problem> = validation
        .stages
        .iter()
        .filter(|stage| stage.outcome == "failed" || stage.outcome == "timed out")
        .map(|stage| {
            let category = ProblemCategory::classify(&stage.stage, stage.detail.as_deref());
            Problem {
                category,
                summary: if stage.outcome == "timed out" {
                    format!("{} timed out", stage.stage)
                } else {
                    stage.stage.clone()
                },
                location: stage.detail.as_deref().and_then(parse_location),
                detail: stage.detail.clone(),
            }
        })
        .collect();
    problems.sort_by_key(|problem| problem.category);
    problems
}

/// Find a `path:line` reference in a compiler or test message.
fn parse_location(detail: &str) -> Option<(String, usize)> {
    for token in detail.split_whitespace() {
        let token = token.trim_matches(|c: char| c == ',' || c == '(' || c == ')');
        let mut parts = token.rsplitn(3, ':');
        let (Some(_column_or_line), Some(second)) = (parts.next(), parts.next()) else {
            continue;
        };
        // `src/main.rs:12:5` and `src/main.rs:12` both end in a number.
        let (path, line) = match parts.next() {
            Some(path) => (path, second.parse::<usize>().ok()),
            None => (
                second,
                token.rsplit(':').next().and_then(|n| n.parse().ok()),
            ),
        };
        if let Some(line) = line
            && path.contains(['/', '.'])
            && !path.is_empty()
        {
            return Some((path.to_owned(), line));
        }
    }
    None
}

#[derive(Clone, Debug)]
pub struct ValidationStage {
    pub stage: String,
    pub outcome: String,
    pub detail: Option<String>,
}

impl ValidationStage {
    /// Only `passed` is success. `unavailable` and `skipped` are their own
    /// outcomes precisely so a client cannot render them green (PRD §24.3).
    pub fn passed(&self) -> bool {
        self.outcome == "passed"
    }
}

#[derive(Clone, Debug, Default)]
pub struct Usage {
    pub total_tokens: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub model_calls: u64,
    pub search_requests: u64,
    pub mcp_calls: u64,
    pub estimated_cost: Option<String>,
    /// Tokens served from the provider's prompt cache.
    pub cache_read_tokens: u64,
    /// Tokens written into the provider's prompt cache.
    pub cache_write_tokens: u64,
    /// Total wall-clock the model calls took, summed across the session.
    pub total_latency_ms: u64,
    /// The coding-worker model's actual context window, when the daemon
    /// resolved one. `None` means unresolved, never "assume 200K".
    pub model_capacity_tokens: Option<u64>,
    /// The most recent turn's actual prompt size — what the *next* request
    /// would cost. `None` before any turn has a recorded ledger entry.
    pub current_context_tokens: Option<u64>,
    /// The model's context window minus the daemon's reserved-output
    /// budget — what a turn can actually fill before compaction kicks in.
    pub effective_capacity_tokens: Option<u64>,
}

impl Usage {
    /// The one-line form for the completion card (PRD §12.11).
    pub fn compact(&self) -> String {
        let tokens = if self.total_tokens >= 1000 {
            format!("{}K tokens", self.total_tokens / 1000)
        } else {
            format!("{} tokens", self.total_tokens)
        };
        let search = match self.search_requests {
            0 => "no web search".to_owned(),
            1 => "1 web search".to_owned(),
            n => format!("{n} web searches"),
        };
        format!("{tokens} · {} model calls · {search}", self.model_calls)
    }

    /// The richer status-bar form: tokens, cache hit, elapsed time.
    ///
    /// Compact enough for a single line, informative enough that the cache
    /// hit-rate and the cost of a run are visible without opening anything.
    pub fn statline(&self) -> String {
        let tokens = if self.total_tokens >= 1000 {
            format!("{:.1}K", self.total_tokens as f64 / 1000.0)
        } else {
            format!("{}", self.total_tokens)
        };
        let mut parts = vec![format!("{tokens} tok")];
        // A cache hit rate only when there is something to rate: a session with
        // zero cached tokens has no "0%" to advertise.
        let cache_total = self.cache_read_tokens + self.cache_write_tokens;
        if cache_total > 0 {
            let rate = self.cache_read_tokens as f64 / cache_total as f64 * 100.0;
            parts.push(format!("cache {rate:.0}%"));
        }
        if self.total_latency_ms > 0 {
            let secs = self.total_latency_ms as f64 / 1000.0;
            parts.push(if secs >= 60.0 {
                format!("{:.0}m{:.0}s", secs / 60.0, secs % 60.0)
            } else {
                format!("{secs:.0}s")
            });
        }
        if let Some(cost) = &self.estimated_cost {
            parts.push(format!("${cost}"));
        }
        parts.join(" · ")
    }
}

/// The adaptive controls in force, as the daemon reports them.
#[derive(Clone, Debug)]
pub struct Controls {
    pub workflow: String,
    pub search: String,
    pub budget: String,
    pub routing: String,
    pub execution_style: String,
    /// The lanes of the chosen plan, when one was built.
    pub lanes: Vec<ActivityLine>,
    pub profile: Option<String>,
}

impl Default for Controls {
    fn default() -> Self {
        Self {
            workflow: "Auto".into(),
            search: "Auto".into(),
            budget: "Balanced".into(),
            routing: "Auto".into(),
            execution_style: "Autonomous".into(),
            lanes: Vec::new(),
            profile: None,
        }
    }
}

/// Everything one session view needs.
#[derive(Clone, Debug, Default)]
pub struct Session {
    pub id: String,
    pub objective: String,
    pub repository: String,
    pub branch: Option<String>,
    pub model: Option<String>,
    pub task_mode: String,
    pub permission_mode: String,
    pub state: Option<ProductStateView>,
    pub plan: Vec<String>,
    pub tasks: Vec<WorkTask>,
    pub current_task: Option<String>,
    pub coverage: Vec<CoverageRow>,
    pub awaiting_plan_review: bool,
    pub recovery_reconciled: bool,
    pub messages: Vec<Message>,
    pub activity: Vec<ActivityLine>,
    pub artifacts: Vec<Artifact>,
    pub changes: Changes,
    pub validation: Validation,
    pub usage: Usage,
    pub controls: Controls,
    pub github_connected: bool,
    pub pull_request: Option<String>,
    /// Per-panel transport provenance. The derived fields above remain
    /// convenient for rendering, while this map prevents an error or an
    /// unavailable route from looking like an empty successful result.
    pub panel_states: BTreeMap<PanelKind, PanelResult>,
}

impl Session {
    /// The canonical state to draw. Falls back to `Ready` rather than inventing
    /// a fourteenth label when the daemon has not classified the session yet.
    pub fn state_view(&self) -> ProductStateView {
        self.state
            .clone()
            .unwrap_or_else(|| ProductStateView::new(ProductState::Ready))
    }

    pub fn title(&self) -> &str {
        let trimmed = self.objective.trim();
        if trimmed.is_empty() {
            "Untitled session"
        } else {
            trimmed
        }
    }

    pub fn panel(&self, kind: PanelKind) -> PanelResult {
        self.panel_states.get(&kind).cloned().unwrap_or_default()
    }

    /// A terminal/uncertain session that left files behind is recoverable
    /// work, not a successful completion. Keep this gate next to the parsed
    /// canonical outcome so every card can use the same truth.
    pub fn has_partial_changes(&self) -> bool {
        self.changes.available
            && self.changes.files_changed > 0
            && matches!(
                self.state_view().state,
                ProductState::Failed | ProductState::Cancelled | ProductState::NeedsRecovery
            )
    }

    pub fn has_successful_outcome(&self) -> bool {
        matches!(
            self.state_view().state,
            ProductState::ReadyForReview | ProductState::Completed
        )
    }
}

// ── Parsing ────────────────────────────────────────────────────────────

fn text(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .filter(|s| !s.is_empty())
}

fn number(value: &Value, key: &str) -> u64 {
    value.get(key).and_then(Value::as_u64).unwrap_or_default()
}

/// Parse a `TurnId` the daemon stamped onto this record, when it stamped
/// one.
///
/// `None` — never a freshly synthesized id — when the field is absent or
/// unparseable: a synthesized id would coincidentally never match a real
/// turn, but claiming that as a positive "no turn" fact rather than
/// "unknown" invites exactly the kind of silent-mismatch bug it should
/// prevent. `work_log_anchor` treats `None` as "no anchor" and renders the
/// work log at the end of the transcript, the same honest fallback used for
/// a transcript with no request in it.
fn turn_id(value: &Value, key: &str) -> Option<TurnId> {
    value
        .get(key)
        .and_then(Value::as_str)
        .and_then(|raw| Uuid::parse_str(raw).ok())
        .map(TurnId)
}

/// Parse an optional `SpanId` the daemon stamped onto this record. Absent
/// today for the same reason `turn_id` is (see [`turn_id`]); `None` here is
/// simply "not available", not a guess.
fn span_id(value: &Value, key: &str) -> Option<SpanId> {
    value
        .get(key)
        .and_then(Value::as_str)
        .and_then(|raw| Uuid::parse_str(raw).ok())
        .map(SpanId)
}

/// Map whatever status word the daemon uses onto a canonical state.
///
/// The daemon owns the vocabulary; this only recognises it. An unknown word
/// becomes `Ready` rather than being shown to the user raw, which is exactly
/// what PRD §13 forbids.
pub fn product_state(word: &str) -> ProductState {
    match word
        .trim()
        .to_ascii_lowercase()
        .replace(['_', '-'], " ")
        .as_str()
    {
        "thinking" => ProductState::Thinking,
        "plan ready" | "planready" => ProductState::PlanReady,
        "working" | "active" | "executing" => ProductState::Working,
        "running command" | "runningcommand" => ProductState::RunningCommand,
        "testing" | "validating" => ProductState::Testing,
        "repairing" => ProductState::Repairing,
        "permission required" | "awaiting approval" | "awaitingapproval" => {
            ProductState::PermissionRequired
        }
        "ready for review" | "awaiting review" | "awaitingreview" => ProductState::ReadyForReview,
        "completed" | "complete" => ProductState::Completed,
        "failed" => ProductState::Failed,
        "cancelled" | "canceled" => ProductState::Cancelled,
        "needs recovery" | "uncertain" | "unavailable" => ProductState::NeedsRecovery,
        _ => ProductState::Ready,
    }
}

/// Parse the session list, keeping only what belongs to `repository`.
///
/// The daemon already filters, but an older daemon does not, and showing one
/// folder's sessions under another folder's name is worse than showing none:
/// the titles look plausible and opening one moves the user to a different
/// project without saying so.
pub fn parse_session_rows_for(raw: &[Value], repository: &std::path::Path) -> Vec<SessionRow> {
    parse_session_rows(raw)
        .into_iter()
        .filter(|row| match row.repository.as_deref() {
            Some(path) => same_folder(std::path::Path::new(path), repository),
            // A row that does not say where it belongs cannot be attributed to
            // this folder, so it is not shown in it.
            None => false,
        })
        .collect()
}

/// Whether two paths name the same folder, resolving links where possible.
fn same_folder(left: &std::path::Path, right: &std::path::Path) -> bool {
    if left == right {
        return true;
    }
    match (left.canonicalize(), right.canonicalize()) {
        (Ok(left), Ok(right)) => left == right,
        _ => false,
    }
}

pub fn parse_session_rows(raw: &[Value]) -> Vec<SessionRow> {
    raw.iter()
        .map(|value| {
            let state = product_state(
                value
                    .get("status_code")
                    .or_else(|| value.get("status"))
                    .and_then(Value::as_str)
                    .unwrap_or(""),
            );
            let needs_attention = value
                .get("needs_attention")
                .and_then(Value::as_bool)
                .unwrap_or(false)
                || matches!(
                    state,
                    ProductState::PermissionRequired | ProductState::NeedsRecovery
                );
            SessionRow {
                id: text(value, "id").unwrap_or_default(),
                repository: text(value, "repository"),
                title: text(value, "title")
                    .or_else(|| text(value, "objective"))
                    .unwrap_or_else(|| "Untitled session".to_owned()),
                state,
                relative_time: text(value, "relative_time").unwrap_or_default(),
                group: session_group(value),
                unread: value
                    .get("unread")
                    .and_then(Value::as_bool)
                    .unwrap_or(false)
                    || needs_attention,
                needs_attention,
                archived: value
                    .get("archived")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
                pinned: value
                    .get("pinned")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
                parent_id: text(value, "parent_id"),
            }
        })
        .filter(|row| !row.id.is_empty())
        .collect()
}

/// An RFC 3339 timestamp as a short, human-readable time.
///
/// A timestamp that cannot be parsed is returned unchanged rather than
/// replaced with a placeholder: showing the raw value lets a user recognise a
/// format problem, where "unknown" hides it.
pub fn relative_time(timestamp: &str) -> String {
    let Ok(parsed) = DateTime::parse_from_rfc3339(timestamp) else {
        return timestamp.to_owned();
    };
    let parsed = parsed.with_timezone(&Utc);
    let now = Utc::now();
    let elapsed = now.signed_duration_since(parsed);
    if elapsed.num_seconds() < 60 {
        return "just now".into();
    }
    if elapsed.num_minutes() < 60 {
        return format!("{}m ago", elapsed.num_minutes());
    }
    if elapsed.num_hours() < 24 {
        return format!("{}h ago", elapsed.num_hours());
    }
    if elapsed.num_days() < 7 {
        return format!("{}d ago", elapsed.num_days());
    }
    parsed.format("%b %-d").to_string()
}

fn session_group(value: &Value) -> String {
    let relative = text(value, "relative_time").unwrap_or_default();
    let normalized = relative.to_ascii_lowercase();
    if normalized.contains("today") || normalized.contains("now") || relative.contains(':') {
        return "Today".into();
    }
    if normalized.contains("yesterday") {
        return "Yesterday".into();
    }
    for key in ["updated_at", "created_at"] {
        if let Some(timestamp) = text(value, key)
            && let Ok(parsed) = DateTime::parse_from_rfc3339(&timestamp)
        {
            let date = parsed.with_timezone(&Utc).date_naive();
            let today = Utc::now().date_naive();
            if date == today {
                return "Today".into();
            }
            if date == today - chrono::Days::new(1) {
                return "Yesterday".into();
            }
            return date.format("%b %-d").to_string();
        }
    }
    if !relative.is_empty() {
        return relative;
    }
    "Earlier".into()
}

fn parse_activity(raw: &[Value]) -> Vec<ActivityLine> {
    raw.iter()
        .map(|value| ActivityLine {
            label: text(value, "label").unwrap_or_else(|| "Working".to_owned()),
            status: text(value, "status").unwrap_or_else(|| "pending".to_owned()),
            summary: text(value, "summary").or_else(|| text(value, "detail")),
            turn_id: turn_id(value, "turn_id"),
            span_id: span_id(value, "span_id"),
            parent_span_id: span_id(value, "parent_span_id"),
        })
        .collect()
}

fn parse_artifacts(raw: &[Value]) -> Vec<Artifact> {
    raw.iter()
        .map(|value| Artifact {
            kind: text(value, "kind").unwrap_or_else(|| "artifact".to_owned()),
            title: text(value, "title").unwrap_or_default(),
            summary: text(value, "summary").unwrap_or_default(),
            steps: value
                .get("steps")
                .and_then(Value::as_array)
                .map(|steps| parse_activity(steps))
                .unwrap_or_default(),
            files: value
                .get("files")
                .and_then(Value::as_array)
                .map(|files| {
                    files
                        .iter()
                        .filter_map(|file| {
                            file.as_str()
                                .map(str::to_owned)
                                .or_else(|| text(file, "path"))
                        })
                        .collect()
                })
                .unwrap_or_default(),
            actions: value
                .get("actions")
                .and_then(Value::as_array)
                .map(|actions| {
                    actions
                        .iter()
                        .filter_map(|action| {
                            Some(ArtifactAction {
                                label: text(action, "label")?,
                                command: text(action, "command").unwrap_or_default(),
                            })
                        })
                        .collect()
                })
                .unwrap_or_default(),
        })
        .filter(|artifact| !artifact.title.is_empty())
        .collect()
}

pub fn parse_changes(raw: &Value) -> Changes {
    if raw.is_null() {
        return Changes::default();
    }
    let status = raw.get("status").and_then(Value::as_str).unwrap_or("ready");
    let entries = raw
        .get("entries")
        .and_then(Value::as_array)
        .map(|entries| {
            entries
                .iter()
                .filter_map(|entry| {
                    Some(ChangedFile {
                        path: text(entry, "path")?,
                        status: text(entry, "status")
                            .and_then(|status| status.chars().next())
                            .unwrap_or('M'),
                        additions: entry
                            .get("additions")
                            .and_then(Value::as_u64)
                            .map(|value| value as usize),
                        deletions: entry
                            .get("deletions")
                            .and_then(Value::as_u64)
                            .map(|value| value as usize),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    Changes {
        scope: match raw.get("scope").and_then(Value::as_str) {
            Some("working_tree") => ChangeScope::WorkingTree,
            Some("staged") => ChangeScope::Staged,
            _ => ChangeScope::Agent,
        },
        entries,
        files_changed: number(raw, "files_changed") as usize,
        additions: number(raw, "additions") as usize,
        deletions: number(raw, "deletions") as usize,
        files: raw
            .get("files")
            .and_then(Value::as_array)
            .map(|files| {
                files
                    .iter()
                    .filter_map(|file| file.as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default(),
        available: status != "unavailable",
    }
}

fn parse_validation(raw: &Value) -> Validation {
    if raw.is_null() {
        return Validation {
            headline: "No validation has run".to_owned(),
            ..Validation::default()
        };
    }
    let stages: Vec<ValidationStage> = raw
        .get("stages")
        .and_then(Value::as_array)
        .map(|stages| {
            stages
                .iter()
                .map(|stage| ValidationStage {
                    stage: text(stage, "stage").unwrap_or_default(),
                    outcome: text(stage, "outcome").unwrap_or_else(|| "unavailable".to_owned()),
                    detail: text(stage, "detail"),
                })
                .collect()
        })
        .unwrap_or_default();
    let complete = raw
        .get("complete")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let headline = if stages.is_empty() {
        "No validation has run".to_owned()
    } else if complete {
        format!("{} validation stage(s) passed", stages.len())
    } else {
        let unresolved = stages.iter().filter(|stage| !stage.passed()).count();
        format!("{unresolved} of {} stage(s) did not pass", stages.len())
    };
    Validation {
        stages,
        complete,
        headline,
    }
}

fn parse_usage(raw: &Value) -> Usage {
    if raw.is_null() {
        return Usage::default();
    }
    Usage {
        total_tokens: number(raw, "total_tokens"),
        input_tokens: number(raw, "input_tokens"),
        output_tokens: number(raw, "output_tokens"),
        model_calls: number(raw, "model_call_count"),
        search_requests: number(raw, "search_requests"),
        mcp_calls: number(raw, "mcp_calls"),
        estimated_cost: raw
            .get("estimated_total_cost")
            .and_then(|cost| {
                cost.as_str()
                    .map(str::to_owned)
                    .or_else(|| cost.as_f64().map(|value| format!("{value:.4}")))
            })
            .filter(|cost| cost != "0.0000"),
        cache_read_tokens: number(raw, "cache_read_tokens"),
        cache_write_tokens: number(raw, "cache_write_tokens"),
        total_latency_ms: number(raw, "total_latency_ms"),
        model_capacity_tokens: raw.get("context_capacity_tokens").and_then(Value::as_u64),
        current_context_tokens: raw.get("current_context_tokens").and_then(Value::as_u64),
        effective_capacity_tokens: raw.get("effective_capacity_tokens").and_then(Value::as_u64),
    }
}

fn titlecase(word: &str) -> String {
    let mut chars = word.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

fn parse_controls(raw: &Value) -> Controls {
    if raw.is_null() {
        return Controls::default();
    }
    let controls = raw.get("controls").unwrap_or(&Value::Null);
    let plan = raw.get("workflow_plan");
    let profile = plan
        .and_then(|plan| text(plan, "profile"))
        .map(|p| titlecase(&p));
    let lanes = plan
        .and_then(|plan| plan.get("lanes"))
        .and_then(Value::as_array)
        .map(|lanes| {
            lanes
                .iter()
                .map(|lane| ActivityLine {
                    label: text(lane, "objective").unwrap_or_else(|| "Lane".to_owned()),
                    status: text(lane, "status").unwrap_or_else(|| "pending".to_owned()),
                    summary: text(lane, "kind"),
                    turn_id: turn_id(lane, "turn_id"),
                    span_id: span_id(lane, "span_id"),
                    parent_span_id: span_id(lane, "parent_span_id"),
                })
                .collect()
        })
        .unwrap_or_default();
    // An absent search policy means "follow the profile default", so resolve it
    // rather than showing a blank control the user cannot interpret.
    let search = text(controls, "search_policy").unwrap_or_else(|| match profile.as_deref() {
        Some("Direct") => "off".to_owned(),
        _ => "auto".to_owned(),
    });
    Controls {
        workflow: titlecase(&text(controls, "workflow").unwrap_or_else(|| "auto".into())),
        search: titlecase(&search),
        budget: titlecase(
            &text(controls, "budget_profile")
                .unwrap_or_else(|| "balanced".into())
                .replace('_', " "),
        ),
        routing: titlecase(&text(controls, "routing").unwrap_or_else(|| "auto".into())),
        execution_style: titlecase(
            &text(controls, "execution_style").unwrap_or_else(|| "autonomous".into()),
        ),
        lanes,
        profile,
    }
}

fn parse_tasks(raw: &Value) -> (Vec<WorkTask>, Option<String>) {
    let raw = raw.get("data").unwrap_or(raw);
    let graph = raw.get("task_graph").unwrap_or(raw);
    let current_task = text(graph, "current_task");
    let tasks = graph
        .get("tasks")
        .and_then(Value::as_array)
        .map(|tasks| {
            tasks
                .iter()
                .filter_map(|task| {
                    Some(WorkTask {
                        id: text(task, "id")?,
                        objective: text(task, "objective").unwrap_or_default(),
                        status: text(task, "status").unwrap_or_else(|| "pending".into()),
                        current_activity: text(task, "current_activity"),
                        evidence_summary: text(task, "evidence_summary"),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    (tasks, current_task)
}

fn parse_coverage(raw: &Value) -> Vec<CoverageRow> {
    fn rows_from_criteria(criteria: &[Value], output: &mut Vec<CoverageRow>) {
        for criterion in criteria {
            if let Some(id) = text(criterion, "id") {
                output.push(CoverageRow {
                    id,
                    statement: text(criterion, "statement").unwrap_or_default(),
                    status: text(criterion, "status").unwrap_or_else(|| "not_run".into()),
                    summary: text(criterion, "summary"),
                });
            }
        }
    }
    let raw = raw.get("data").unwrap_or(raw);
    let mut rows = Vec::new();
    match raw {
        Value::Array(values) => rows_from_criteria(values, &mut rows),
        Value::Object(_) => {
            if let Some(values) = raw.get("evidence").and_then(Value::as_array) {
                rows_from_criteria(values, &mut rows);
            }
            if let Some(values) = raw.get("coverage").and_then(Value::as_array) {
                rows_from_criteria(values, &mut rows);
            }
            if let Some(requirements) = raw.get("requirements").and_then(Value::as_array) {
                for requirement in requirements {
                    if let Some(criteria) = requirement.get("criteria").and_then(Value::as_array) {
                        rows_from_criteria(criteria, &mut rows);
                    }
                }
            }
        }
        _ => {}
    }
    rows
}

/// Build the full view model from one daemon snapshot.
pub fn parse_session(id: &str, snapshot: &crate::daemon::SessionSnapshot) -> Session {
    let summary = &snapshot.summary;
    let state_word = summary
        .get("state")
        .and_then(Value::as_str)
        .or_else(|| summary.get("status").and_then(Value::as_str))
        .unwrap_or("");
    let awaiting_plan_review = summary
        .get("awaiting_plan_review")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let recovery_reconciled = summary
        .get("recovery_reconciled")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    // A session paused on an untouched plan is "Plan ready", whatever the raw
    // lifecycle status says. Showing `paused blocked` here is the exact failure
    // PRD §13 calls out.
    let state = if awaiting_plan_review {
        ProductState::PlanReady
    } else {
        product_state(state_word)
    };

    let (tasks, current_task) = parse_tasks(&snapshot.tasks);
    let coverage = parse_coverage(&snapshot.spec);
    let coverage = if coverage.is_empty() {
        parse_coverage(&snapshot.evidence)
    } else {
        coverage
    };
    Session {
        id: id.to_owned(),
        objective: text(summary, "objective").unwrap_or_default(),
        repository: text(summary, "repository").unwrap_or_default(),
        branch: text(summary, "branch"),
        model: text(summary, "model").or_else(|| text(summary, "selected_model")),
        task_mode: titlecase(&text(summary, "task_mode").unwrap_or_else(|| "ask".into())),
        permission_mode: text(summary, "permission_mode")
            .map(|mode| titlecase(&mode.replace('-', " ")))
            .unwrap_or_else(|| "Ask".to_owned()),
        state: Some(ProductStateView::new(state)),
        plan: summary
            .get("plan")
            .and_then(Value::as_array)
            .map(|steps| {
                steps
                    .iter()
                    .filter_map(|step| step.as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default(),
        tasks,
        current_task,
        coverage,
        awaiting_plan_review,
        recovery_reconciled,
        messages: snapshot
            .conversation
            .iter()
            .filter_map(|value| {
                Some(Message {
                    id: text(value, "id").unwrap_or_default(),
                    role: text(value, "role")?,
                    content: text(value, "content").unwrap_or_default(),
                    timestamp: text(value, "timestamp").unwrap_or_default(),
                    turn_id: turn_id(value, "turn_id"),
                    span_id: span_id(value, "span_id"),
                    parent_span_id: span_id(value, "parent_span_id"),
                })
            })
            .collect(),
        activity: parse_activity(&snapshot.activity),
        artifacts: parse_artifacts(&snapshot.artifacts),
        changes: parse_changes(&snapshot.changes),
        validation: parse_validation(&snapshot.validation),
        usage: parse_usage(&snapshot.usage),
        controls: parse_controls(&snapshot.controls),
        github_connected: snapshot
            .github
            .get("connected")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        pull_request: text(&snapshot.github, "pr_url"),
        panel_states: snapshot.panels.clone(),
    }
}

/// Apply one independently fetched presentation panel to an existing session.
///
/// `LoadSession` deliberately fans out into bounded panel requests. The first
/// response is only a loading skeleton, so every later panel must update both
/// its provenance and the typed view model; updating provenance alone left a
/// successfully loaded history looking permanently empty.
pub fn apply_panel(session: &mut Session, kind: PanelKind, result: PanelResult) {
    let mut snapshot = crate::daemon::SessionSnapshot::default();
    snapshot.set_panel(kind, result.clone());
    let parsed = parse_session(&session.id, &snapshot);
    match kind {
        PanelKind::Summary => {
            session.objective = parsed.objective;
            session.repository = parsed.repository;
            session.branch = parsed.branch;
            session.model = parsed.model;
            session.task_mode = parsed.task_mode;
            session.permission_mode = parsed.permission_mode;
            session.state = parsed.state;
            session.plan = parsed.plan;
            session.awaiting_plan_review = parsed.awaiting_plan_review;
            session.recovery_reconciled = parsed.recovery_reconciled;
        }
        PanelKind::Conversation => session.messages = parsed.messages,
        PanelKind::Activity => session.activity = parsed.activity,
        PanelKind::Artifacts => session.artifacts = parsed.artifacts,
        PanelKind::Changes => session.changes = parsed.changes,
        PanelKind::Validation => session.validation = parsed.validation,
        PanelKind::Usage => session.usage = parsed.usage,
        PanelKind::Controls => session.controls = parsed.controls,
        PanelKind::Github => {
            session.github_connected = parsed.github_connected;
            session.pull_request = parsed.pull_request;
        }
        PanelKind::Spec => session.coverage = parsed.coverage,
        PanelKind::Tasks => {
            session.tasks = parsed.tasks;
            session.current_task = parsed.current_task;
        }
        PanelKind::Evidence => {
            if session.coverage.is_empty() {
                session.coverage = parsed.coverage;
            }
        }
    }
    session.panel_states.insert(kind, result);
}

/// Human-facing text for an evidence panel's transport state.
pub fn panel_availability_label(availability: &PanelAvailability) -> &'static str {
    match availability {
        PanelAvailability::Loading => "Loading",
        PanelAvailability::Ready => "Ready",
        PanelAvailability::Empty => "Empty",
        PanelAvailability::Unavailable => "Unavailable",
        PanelAvailability::Error => "Error",
    }
}

// ── Project memory ─────────────────────────────────────────────────────

/// One durable thing PurrCode knows about this project.
///
/// Every field except the content is provenance, and that is the point: this
/// is auditable knowledge, not a black box. A user must be able to ask "why
/// does it believe this?" and get an answer — where it came from, when, how
/// sure it is, and whether anything has used it since.
#[derive(Clone, Debug)]
pub struct MemoryEntry {
    pub id: String,
    /// The bucket it belongs to: build, architecture, learnings, user rules.
    pub kind: String,
    pub content: String,
    /// Where the knowledge came from — a session, a document, the user.
    pub source: String,
    pub confidence: String,
    pub scope: String,
    pub created_at: String,
    /// When something last drew on this. `None` means nothing has, which is
    /// worth seeing: unused memory is a candidate for forgetting.
    pub last_used_at: Option<String>,
}

impl MemoryEntry {
    /// Parses the daemon's `{"entries": {kind: [...]}}` shape into a flat,
    /// kind-ordered list.
    pub fn parse_all(value: &Value) -> Vec<Self> {
        let Some(groups) = value["entries"].as_object() else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for (kind, entries) in groups {
            for entry in entries.as_array().unwrap_or(&Vec::new()) {
                let Some(id) = entry["id"].as_str() else {
                    continue;
                };
                out.push(Self {
                    id: id.to_owned(),
                    kind: kind.clone(),
                    content: entry["content"].as_str().unwrap_or_default().to_owned(),
                    source: entry["source"].as_str().unwrap_or_default().to_owned(),
                    confidence: entry["confidence"]
                        .as_str()
                        .unwrap_or("unverified")
                        .to_owned(),
                    scope: entry["scope"].as_str().unwrap_or("repository").to_owned(),
                    created_at: entry["created_at"].as_str().unwrap_or_default().to_owned(),
                    last_used_at: entry["last_used_at"].as_str().map(str::to_owned),
                });
            }
        }
        out
    }

    /// The provenance line shown under every entry.
    pub fn provenance(&self) -> String {
        let used = match &self.last_used_at {
            Some(when) => format!("last used {}", relative_time(when)),
            None => "never used".to_owned(),
        };
        format!(
            "{} · {} · {} · added {} · {used}",
            self.source,
            self.confidence,
            self.scope,
            relative_time(&self.created_at),
        )
    }
}

/// The kinds of project memory, in the order the panel lists them.
pub const MEMORY_KINDS: &[(&str, &str)] = &[
    ("build", "How to build, test, and run this project"),
    ("architecture", "How the project is put together"),
    ("learnings", "Things discovered while working on it"),
    ("user_rules", "Standing instructions from the user"),
];

/// One hit from full-text search across session event logs.
#[derive(Clone, Debug)]
pub struct SessionHit {
    pub session_id: String,
    pub event_type: String,
    pub snippet: String,
    pub occurred_at: String,
}

impl SessionHit {
    pub fn parse_all(value: &Value) -> Vec<Self> {
        value
            .as_array()
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| {
                        Some(Self {
                            session_id: item["session_id"].as_str()?.to_owned(),
                            event_type: item["event_type"].as_str().unwrap_or_default().to_owned(),
                            snippet: item["snippet"].as_str().unwrap_or_default().to_owned(),
                            occurred_at: item["occurred_at"]
                                .as_str()
                                .unwrap_or_default()
                                .to_owned(),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default()
    }
}

// ── Checkpoints ────────────────────────────────────────────────────────

/// One restorable point in a session's history.
#[derive(Clone, Debug)]
pub struct Checkpoint {
    pub id: String,
    pub label: String,
    /// The base commit the checkpoint's patch applies over.
    pub head: String,
    pub created_at: String,
    /// Files the checkpoint's patch touches, from the preview route. `None`
    /// until the preview has been fetched — which is not the same as a
    /// checkpoint that changes nothing, so the dialog waits rather than
    /// claiming "0 files".
    pub changed_files: Option<Vec<String>>,
}

impl Checkpoint {
    pub fn parse_all(value: &Value) -> Vec<Self> {
        value
            .as_array()
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| {
                        Some(Self {
                            id: item["id"].as_str()?.to_owned(),
                            label: item["label"].as_str().unwrap_or_default().to_owned(),
                            head: item["head"].as_str().unwrap_or_default().to_owned(),
                            created_at: item["created_at"].as_str().unwrap_or_default().to_owned(),
                            changed_files: None,
                        })
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// A short label for a row: the checkpoint's own label, or its head when
    /// it has none.
    pub fn display(&self) -> String {
        if self.label.trim().is_empty() {
            let head: String = self.head.chars().take(8).collect();
            format!("Checkpoint {head}")
        } else {
            self.label.clone()
        }
    }
}

/// What a restore should put back.
///
/// The conversation and the worktree are separate stores, so restoring one
/// without the other is a real and sometimes wanted operation — rewinding the
/// code while keeping what was said about it, for instance.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RestoreScope {
    ConversationOnly,
    CodeOnly,
    Both,
}

impl RestoreScope {
    pub const ALL: &'static [Self] = &[Self::Both, Self::CodeOnly, Self::ConversationOnly];

    pub const fn label(self) -> &'static str {
        match self {
            Self::ConversationOnly => "Conversation only",
            Self::CodeOnly => "Code only",
            Self::Both => "Conversation and code",
        }
    }

    pub const fn description(self) -> &'static str {
        match self {
            Self::ConversationOnly => {
                "Fork the conversation at this point. The worktree is left as it is."
            }
            Self::CodeOnly => {
                "Restore the worktree to this checkpoint. The conversation is left as it is."
            }
            Self::Both => "Restore the worktree and fork the conversation at this point.",
        }
    }

    /// Whether this scope touches the worktree.
    pub const fn restores_code(self) -> bool {
        matches!(self, Self::CodeOnly | Self::Both)
    }

    /// Whether this scope forks the conversation.
    pub const fn forks_conversation(self) -> bool {
        matches!(self, Self::ConversationOnly | Self::Both)
    }
}

/// One composer reference, as the daemon resolved it.
///
/// `resolved` is the daemon's answer, not an inference from whether a preview
/// came back: a reference can resolve to genuinely empty content, and treating
/// that as failure would tell the user their file was not found.
#[derive(Clone, Debug)]
pub struct ResolvedReference {
    pub display: String,
    pub resolved: bool,
    pub preview: Option<String>,
    /// Why it did not resolve, in the daemon's words.
    pub diagnostics: Option<String>,
}

impl ResolvedReference {
    pub fn parse_all(value: &Value) -> Vec<Self> {
        value
            .as_array()
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| {
                        Some(Self {
                            display: item["display"].as_str()?.to_owned(),
                            resolved: item["resolved"].as_bool().unwrap_or(false),
                            preview: item["preview"].as_str().map(str::to_owned),
                            diagnostics: item["diagnostics"].as_str().map(str::to_owned),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default()
    }
}

// ── Language intelligence (LSP) ────────────────────────────────────────

/// A 0-based position in a document, matching the LSP wire shape.
///
/// The editor thinks in 1-based line numbers because that is what a gutter
/// shows; everything crossing the daemon boundary stays 0-based so there is
/// exactly one place (`Location::display_line`) where the conversion happens.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct DocumentPosition {
    pub line: u64,
    pub character: u64,
}

impl DocumentPosition {
    pub fn parse(value: &Value) -> Self {
        Self {
            line: value["line"].as_u64().unwrap_or(0),
            character: value["character"].as_u64().unwrap_or(0),
        }
    }

    /// The line as a gutter shows it.
    pub fn display_line(self) -> usize {
        self.line as usize + 1
    }
}

/// A place a language server pointed at: a definition, a reference, or a
/// symbol's home.
#[derive(Clone, Debug)]
pub struct Location {
    pub path: std::path::PathBuf,
    pub start: DocumentPosition,
}

impl Location {
    /// Parses one `LocationLink`. Returns `None` for an entry with no usable
    /// target, so a malformed element drops out instead of pointing the user
    /// at line 1 of nothing.
    pub fn parse(value: &Value) -> Option<Self> {
        let uri = value["target_uri"].as_str()?;
        let path = uri.strip_prefix("file://").unwrap_or(uri);
        if path.is_empty() {
            return None;
        }
        Some(Self {
            path: std::path::PathBuf::from(path),
            // Prefer the selection range: it is the identifier itself, where
            // the full target range can be an entire function body.
            start: DocumentPosition::parse(if value["target_selection_range"].is_object() {
                &value["target_selection_range"]["start"]
            } else {
                &value["target_range"]["start"]
            }),
        })
    }

    /// `path:line`, repository-relative when possible.
    pub fn label(&self, repository: &std::path::Path) -> String {
        let shown = self.path.strip_prefix(repository).unwrap_or(&self.path);
        format!("{}:{}", shown.display(), self.start.display_line())
    }
}

/// One symbol in the open document's outline.
#[derive(Clone, Debug)]
pub struct Symbol {
    pub name: String,
    pub kind: u64,
    pub detail: Option<String>,
    pub start: DocumentPosition,
}

impl Symbol {
    pub fn parse(value: &Value) -> Option<Self> {
        Some(Self {
            name: value["name"].as_str()?.to_owned(),
            kind: value["kind"].as_u64().unwrap_or(0),
            detail: value["detail"].as_str().map(str::to_owned),
            start: DocumentPosition::parse(&value["selection_range"]["start"]),
        })
    }

    /// The LSP `SymbolKind` as a short word. Unknown kinds render as "symbol"
    /// rather than as a number the user would have to look up.
    pub const fn kind_label(&self) -> &'static str {
        match self.kind {
            2 => "module",
            5 => "class",
            6 => "method",
            8 => "field",
            9 => "constructor",
            10 => "enum",
            11 => "interface",
            12 => "function",
            13 => "variable",
            14 => "constant",
            23 => "struct",
            26 => "type",
            _ => "symbol",
        }
    }
}

/// One diagnostic a language server published for a file.
#[derive(Clone, Debug)]
pub struct Diagnostic {
    pub start: DocumentPosition,
    /// LSP severity: 1 error, 2 warning, 3 information, 4 hint. `None` when
    /// the server omitted it.
    pub severity: Option<u64>,
    pub code: Option<String>,
    pub source: Option<String>,
    pub message: String,
}

impl Diagnostic {
    pub fn parse(value: &Value) -> Self {
        Self {
            start: DocumentPosition::parse(&value["range"]["start"]),
            severity: value["severity"].as_u64(),
            code: value["code"].as_str().map(str::to_owned),
            source: value["source"].as_str().map(str::to_owned),
            message: value["message"].as_str().unwrap_or_default().to_owned(),
        }
    }

    /// A server that omitted severity has not said the problem is minor, so an
    /// unlabelled diagnostic sorts with errors rather than being quietly
    /// filed as a hint.
    pub const fn is_error(&self) -> bool {
        matches!(self.severity, Some(1) | None)
    }

    pub const fn severity_label(&self) -> &'static str {
        match self.severity {
            Some(1) => "Error",
            Some(2) => "Warning",
            Some(3) => "Info",
            Some(4) => "Hint",
            _ => "Unspecified",
        }
    }
}

/// Everything the IDE knows from language servers right now.
///
/// Every field distinguishes "asked and got nothing" from "never asked".
/// Language servers analyse asynchronously, so an empty diagnostic list moments
/// after opening a file means the server has not spoken yet — rendering that as
/// a clean file would be a lie the user would rely on.
#[derive(Clone, Debug, Default)]
pub struct LanguageIntelligence {
    /// Language servers present on this machine, as `(program, extensions)`.
    pub servers: Vec<(String, Vec<String>)>,
    /// `true` once the server probe has answered at least once.
    pub servers_checked: bool,
    /// Hover text for the position the pointer last rested on.
    pub hover: Option<String>,
    /// The document and position `hover` describes, so a stale reply for a
    /// position the pointer has already left is discarded rather than shown
    /// against the wrong token.
    pub hover_anchor: Option<(std::path::PathBuf, DocumentPosition)>,
    /// Results of the last "find references" request.
    pub references: Vec<Location>,
    pub references_for: Option<String>,
    pub references_checked: bool,
    /// The open document's outline.
    pub symbols: Vec<Symbol>,
    pub symbols_for: Option<std::path::PathBuf>,
    /// Published diagnostics per file.
    pub diagnostics: BTreeMap<std::path::PathBuf, Vec<Diagnostic>>,
    /// `true` once a diagnostics poll has answered. Until then the Problems
    /// panel says the servers are still warming up instead of "no problems".
    pub diagnostics_checked: bool,
    /// The last language-server failure, shown as an explanation rather than
    /// silently producing an empty result.
    pub last_error: Option<String>,
}

impl LanguageIntelligence {
    /// Whether any language server covers this file's extension.
    ///
    /// Used to keep the UI honest: an unsupported file must not offer
    /// "Go to definition" and then do nothing.
    pub fn supports(&self, path: &std::path::Path) -> bool {
        let Some(extension) = path.extension().and_then(|ext| ext.to_str()) else {
            return false;
        };
        let extension = extension.to_ascii_lowercase();
        self.servers
            .iter()
            .any(|(_, extensions)| extensions.iter().any(|candidate| candidate == &extension))
    }

    /// Diagnostics for one file, newest snapshot the daemon published.
    pub fn for_file(&self, path: &std::path::Path) -> &[Diagnostic] {
        self.diagnostics.get(path).map_or(&[], Vec::as_slice)
    }

    /// Total diagnostics across every file, for the status bar.
    pub fn counts(&self) -> (usize, usize) {
        let mut errors = 0;
        let mut others = 0;
        for diagnostics in self.diagnostics.values() {
            for diagnostic in diagnostics {
                if diagnostic.is_error() {
                    errors += 1;
                } else {
                    others += 1;
                }
            }
        }
        (errors, others)
    }

    /// Replaces the whole diagnostic set from a `GET /v1/lsp/diagnostics` body.
    pub fn absorb_diagnostics(&mut self, value: &Value) {
        let mut next: BTreeMap<std::path::PathBuf, Vec<Diagnostic>> = BTreeMap::new();
        for file in value["files"].as_array().unwrap_or(&Vec::new()) {
            let Some(path) = file["path"].as_str() else {
                continue;
            };
            let diagnostics: Vec<Diagnostic> = file["diagnostics"]
                .as_array()
                .map(|items| items.iter().map(Diagnostic::parse).collect())
                .unwrap_or_default();
            // A file the server has declared clean is reported with an empty
            // array; dropping the key entirely would be the same shape as
            // "never analysed", which the panel must not conflate.
            next.insert(std::path::PathBuf::from(path), diagnostics);
        }
        self.diagnostics = next;
        self.diagnostics_checked = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::daemon::SessionSnapshot;
    use serde_json::json;

    #[test]
    fn independently_loaded_panels_populate_history_before_the_timeout() {
        let mut session = Session {
            id: "history-1".into(),
            ..Session::default()
        };
        apply_panel(
            &mut session,
            PanelKind::Conversation,
            PanelResult::success(json!([
                {"role": "user", "content": "hello"},
                {"role": "assistant", "content": "Hi from durable history"}
            ])),
        );
        assert_eq!(session.messages.len(), 2);
        assert_eq!(session.messages[1].content, "Hi from durable history");
        assert_eq!(
            session
                .panel_states
                .get(&PanelKind::Conversation)
                .map(|state| &state.availability),
            Some(&PanelAvailability::Ready)
        );
    }

    #[test]
    fn a_plan_awaiting_review_reads_as_plan_ready_not_as_a_paused_session() {
        let snapshot = SessionSnapshot {
            summary: json!({
                "objective": "Add search",
                "status": "paused",
                "awaiting_plan_review": true,
            }),
            ..SessionSnapshot::default()
        };
        let session = parse_session("s1", &snapshot);
        assert_eq!(session.state_view().label, "Plan ready");
        assert_eq!(
            session.state_view().primary_action.as_deref(),
            Some("Build this plan")
        );
    }

    #[test]
    fn recovery_reconciliation_survives_snapshot_parsing() {
        let snapshot = SessionSnapshot {
            summary: json!({
                "objective": "Resume interrupted work",
                "status": "paused",
                "recovery_reconciled": true,
            }),
            ..SessionSnapshot::default()
        };
        let session = parse_session("s1", &snapshot);
        assert!(session.recovery_reconciled);
        assert_eq!(session.state_view().label, "Ready");
    }

    #[test]
    fn an_unknown_status_word_never_reaches_the_screen_verbatim() {
        // The daemon may grow a status this build has not seen. Falling back to
        // a canonical label is what stops `SessionPaused` appearing in the UI.
        let state = product_state("SomeFutureRuntimeState");
        assert_eq!(state, ProductState::Ready);
        assert_eq!(state.label(), "Ready");
    }

    #[test]
    fn runtime_status_words_map_onto_product_vocabulary() {
        assert_eq!(
            product_state("awaiting_approval"),
            ProductState::PermissionRequired
        );
        assert_eq!(
            product_state("awaiting-review"),
            ProductState::ReadyForReview
        );
        assert_eq!(product_state("uncertain"), ProductState::NeedsRecovery);
        assert_eq!(product_state("unavailable"), ProductState::NeedsRecovery);
        assert_eq!(product_state("executing"), ProductState::Working);
    }

    #[test]
    fn no_worktree_means_changes_are_unavailable_not_zero() {
        let changes = parse_changes(&json!({"status": "unavailable", "files_changed": 0}));
        assert!(
            !changes.available,
            "an unchecked repository is not a clean one"
        );
        let ready = parse_changes(&json!({"status": "ready", "files_changed": 3}));
        assert!(ready.available);
        assert_eq!(ready.files_changed, 3);
    }

    #[test]
    fn an_unavailable_validation_stage_is_not_counted_as_passed() {
        let validation = parse_validation(&json!({
            "complete": false,
            "stages": [
                {"stage": "unit tests", "outcome": "passed"},
                {"stage": "integration", "outcome": "unavailable", "detail": "no docker"}
            ]
        }));
        assert!(!validation.complete);
        assert!(validation.stages[0].passed());
        assert!(!validation.stages[1].passed());
        assert!(
            validation.headline.contains("1 of 2"),
            "{}",
            validation.headline
        );
    }

    #[test]
    fn missing_validation_is_reported_as_not_run() {
        assert_eq!(
            parse_validation(&Value::Null).headline,
            "No validation has run"
        );
    }

    #[test]
    fn the_usage_line_says_no_web_search_when_there_was_none() {
        let usage = Usage {
            total_tokens: 42_000,
            model_calls: 6,
            ..Usage::default()
        };
        assert_eq!(
            usage.compact(),
            "42K tokens · 6 model calls · no web search"
        );
    }

    #[test]
    fn parse_usage_reads_the_daemon_resolved_model_capacity() {
        let usage = parse_usage(&json!({
            "total_tokens": 42_000,
            "context_capacity_tokens": 32_000,
        }));
        assert_eq!(usage.model_capacity_tokens, Some(32_000));
    }

    #[test]
    fn parse_usage_leaves_capacity_unknown_when_the_daemon_did_not_resolve_one() {
        let usage = parse_usage(&json!({"total_tokens": 42_000}));
        assert_eq!(usage.model_capacity_tokens, None);
    }

    #[test]
    fn parse_usage_reads_the_current_turn_context_and_effective_capacity() {
        let usage = parse_usage(&json!({
            "total_tokens": 42_000,
            "current_context_tokens": 12_400,
            "effective_capacity_tokens": 23_800,
        }));
        assert_eq!(usage.current_context_tokens, Some(12_400));
        assert_eq!(usage.effective_capacity_tokens, Some(23_800));
    }

    #[test]
    fn parse_usage_leaves_current_context_and_effective_capacity_unknown_when_absent() {
        let usage = parse_usage(&json!({"total_tokens": 42_000}));
        assert_eq!(usage.current_context_tokens, None);
        assert_eq!(usage.effective_capacity_tokens, None);
    }

    #[test]
    fn a_direct_plan_shows_search_off_rather_than_an_empty_control() {
        let controls = parse_controls(&json!({
            "controls": {"workflow": "direct", "budget_profile": "economy"},
            "workflow_plan": {"profile": "direct", "lanes": []}
        }));
        assert_eq!(controls.workflow, "Direct");
        assert_eq!(controls.search, "Off");
        assert_eq!(controls.budget, "Economy");
    }

    #[test]
    fn an_explicit_search_setting_wins_over_the_profile_default() {
        let controls = parse_controls(&json!({
            "controls": {"workflow": "ultra", "search_policy": "off"},
            "workflow_plan": {"profile": "ultra", "lanes": []}
        }));
        assert_eq!(controls.search, "Off");
    }

    #[test]
    fn a_repeated_runtime_step_collapses_into_one_counted_line() {
        let raw: Vec<ActivityLine> = ["Ran a command", "Validation failed"]
            .iter()
            .cycle()
            .take(20)
            .map(|label| ActivityLine {
                label: (*label).to_owned(),
                status: "done".into(),
                summary: None,
                ..Default::default()
            })
            .collect();
        let folded = condense(&raw);
        assert_eq!(folded.len(), 20, "alternating labels do not fold together");

        let same: Vec<ActivityLine> = (0..12)
            .map(|_| ActivityLine {
                label: "Validation failed".into(),
                status: "failed".into(),
                summary: None,
                ..Default::default()
            })
            .collect();
        let folded = condense(&same);
        assert_eq!(folded.len(), 1);
        assert_eq!(folded[0].1, 12, "the count must be preserved, not dropped");
    }

    #[test]
    fn folding_keeps_the_most_recent_detail() {
        let lines = vec![
            ActivityLine {
                label: "Testing".into(),
                status: "failed".into(),
                summary: Some("first".into()),
                ..Default::default()
            },
            ActivityLine {
                label: "Testing".into(),
                status: "failed".into(),
                summary: Some("latest".into()),
                ..Default::default()
            },
        ];
        let folded = condense(&lines);
        assert_eq!(folded[0].0.summary.as_deref(), Some("latest"));
    }

    #[test]
    fn duplicate_context_index_results_are_deduped_without_hiding_changes() {
        let lines = vec![
            ActivityLine {
                label: "Indexed 0 file(s), 0 symbol(s)".into(),
                status: "done".into(),
                summary: None,
                ..Default::default()
            },
            ActivityLine {
                label: "Read repository manifest".into(),
                status: "done".into(),
                summary: None,
                ..Default::default()
            },
            ActivityLine {
                label: "Indexed 0 file(s), 0 symbol(s)".into(),
                status: "done".into(),
                summary: None,
                ..Default::default()
            },
            ActivityLine {
                label: "Indexed 12 file(s), 34 symbol(s)".into(),
                status: "done".into(),
                summary: None,
                ..Default::default()
            },
        ];
        let folded = condense(&lines);
        assert_eq!(
            folded
                .iter()
                .map(|(line, _)| line.label.as_str())
                .collect::<Vec<_>>(),
            vec![
                "Read repository manifest",
                "Indexed 0 file(s), 0 symbol(s)",
                "Indexed 12 file(s), 34 symbol(s)"
            ]
        );
    }

    #[test]
    fn session_rows_flag_the_ones_that_need_a_person() {
        let rows = parse_session_rows(&[
            json!({"id": "a", "objective": "Fix upload bug", "status_code": "awaiting_approval"}),
            json!({"id": "b", "objective": "Add tests", "status_code": "completed"}),
            json!({"objective": "no id, must be dropped"}),
        ]);
        assert_eq!(rows.len(), 2);
        assert!(rows[0].needs_attention);
        assert!(!rows[1].needs_attention);
    }

    #[test]
    fn session_rows_have_reference_date_groups_and_unread_state() {
        let rows = parse_session_rows(&[
            json!({
                "id": "today",
                "objective": "Today task",
                "relative_time": "10:24",
                "unread": true
            }),
            json!({
                "id": "yesterday",
                "objective": "Yesterday task",
                "relative_time": "Yesterday"
            }),
        ]);
        assert_eq!(rows[0].group, "Today");
        assert!(rows[0].unread);
        assert_eq!(rows[1].group, "Yesterday");
    }

    #[test]
    fn an_empty_daemon_snapshot_produces_a_renderable_session() {
        // A daemon that answers nothing must still leave the window drawable.
        let session = parse_session("s", &SessionSnapshot::default());
        assert_eq!(session.title(), "Untitled session");
        assert_eq!(session.state_view().label, "Ready");
        assert!(session.messages.is_empty());
        assert!(!session.changes.available);
    }

    #[test]
    fn panel_provenance_survives_a_failed_snapshot() {
        let mut snapshot = SessionSnapshot::loading();
        snapshot.set_panel(
            PanelKind::Changes,
            PanelResult::failure("/v1/sessions/s/changes returned 503: busy".into()),
        );
        let session = parse_session("s", &snapshot);
        assert_eq!(
            session.panel(PanelKind::Changes).availability,
            PanelAvailability::Error
        );
        assert!(
            session
                .panel(PanelKind::Changes)
                .error
                .as_deref()
                .is_some_and(|detail| detail.contains("503"))
        );
        assert!(!session.changes.available);
    }

    #[test]
    fn failed_changes_are_partial_not_successful_completion() {
        let snapshot = SessionSnapshot {
            summary: json!({"status": "failed", "objective": "Fix bug"}),
            changes: json!({"status": "ready", "files_changed": 2}),
            ..SessionSnapshot::default()
        };
        let session = parse_session("s", &snapshot);
        assert!(session.has_partial_changes());
        assert!(!session.has_successful_outcome());
    }

    #[test]
    fn durable_tasks_and_evidence_feed_plan_and_coverage_models() {
        let snapshot = SessionSnapshot {
            tasks: json!({
                "state": "ready",
                "data": {
                    "current_task": "task-2",
                    "tasks": [{"id": "task-2", "objective": "Run checks", "status": "running"}]
                }
            }),
            spec: json!({
                "state": "ready",
                "data": {"requirements": [{"criteria": [
                    {"id": "criterion-1", "statement": "Checks pass", "status": "covered"}
                ]}]}
            }),
            ..SessionSnapshot::default()
        };
        let session = parse_session("s", &snapshot);
        assert_eq!(session.current_task.as_deref(), Some("task-2"));
        assert_eq!(session.tasks[0].objective, "Run checks");
        assert_eq!(session.coverage[0].status, "covered");
    }

    #[test]
    fn a_change_set_distinguishes_unchecked_from_empty() {
        // PRD §25: "0 files changed" claims a check that never happened.
        let unchecked = parse_changes(&json!({"status": "unavailable"}));
        assert!(!unchecked.available);
        assert_eq!(unchecked.summary(), "Not checked yet");

        let empty = parse_changes(&json!({"status": "ready", "scope": "agent"}));
        assert!(empty.available);
        assert_eq!(empty.summary(), "No agent changes");
    }

    #[test]
    fn changed_files_carry_what_happened_to_them() {
        let changes = parse_changes(&json!({
            "status": "ready",
            "scope": "agent",
            "files_changed": 3,
            "additions": 84,
            "deletions": 21,
            "files": ["a.py", "b.py", "c.py"],
            "entries": [
                {"path": "a.py", "status": "M", "additions": 40, "deletions": 21},
                {"path": "b.py", "status": "A", "additions": 44, "deletions": 0},
                {"path": "c.py", "status": "D"},
            ],
        }));
        assert_eq!(changes.scope, ChangeScope::Agent);
        assert_eq!(changes.summary(), "3 files · +84 −21");
        assert_eq!(changes.entries[1].status_word(), "added");
        assert_eq!(
            changes.entries[2].status_word(),
            "deleted",
            "a deleted file shown as modified sends the reviewer looking for it"
        );
        assert_eq!(changes.entries[2].additions, None);
    }

    #[test]
    fn a_single_changed_file_is_not_pluralised() {
        let changes = parse_changes(&json!({
            "status": "ready", "files_changed": 1, "additions": 2, "deletions": 0,
        }));
        assert_eq!(changes.summary(), "1 file · +2 −0");
    }

    #[test]
    fn the_workspace_route_parses_as_working_tree_scope() {
        // FR-C1: the workspace-changes response is the same shape as the
        // session route minus the worktree block, so `parse_changes` must
        // accept it unchanged and read the counts.
        let changes = parse_changes(&json!({
            "status": "ready",
            "scope": "working_tree",
            "scope_label": "Working tree",
            "files_changed": 2,
            "additions": 84,
            "deletions": 21,
            "files": ["a.py", "b.py"],
            "entries": [
                {"path": "a.py", "status": "M", "additions": 40, "deletions": 21},
                {"path": "b.py", "status": "A", "additions": 44, "deletions": 0},
            ],
        }));
        assert_eq!(changes.scope, ChangeScope::WorkingTree);
        assert_eq!(changes.files_changed, 2);
        assert_eq!(changes.additions, 84);
        assert_eq!(changes.deletions, 21);
        assert_eq!(changes.entries.len(), 2);
        assert_eq!(changes.entries[0].additions, Some(40));
        assert_eq!(changes.entries[1].additions, Some(44));
        assert_eq!(changes.summary(), "2 files · +84 −21");
    }

    #[test]
    fn validation_aggregates_instead_of_repeating_itself() {
        // PRD §29: a wall of identical "Validation failed" lines is not a
        // report.
        let validation = parse_validation(&json!({
            "stages": [
                {"stage": "Syntax and static checks", "outcome": "passed"},
                {"stage": "Focused tests", "outcome": "passed"},
                {"stage": "Full test suite", "outcome": "failed", "detail": "2 failed"},
            ],
        }));
        assert_eq!(
            validation.aggregate().as_deref(),
            Some("2 / 3 checks passed")
        );
        assert!(validation.needs_attention());
        assert_eq!(validation.failed(), 1);

        let clean = parse_validation(&json!({
            "stages": [{"stage": "Focused tests", "outcome": "passed"}],
        }));
        assert_eq!(clean.aggregate().as_deref(), Some("All 1 checks passed"));
        assert!(!clean.needs_attention());

        assert_eq!(
            parse_validation(&json!({"stages": []})).aggregate(),
            None,
            "nothing having run is not the same as everything passing"
        );
    }

    #[test]
    fn a_skipped_stage_is_not_a_problem() {
        // PRD §24.3: only `passed` is success, but `skipped` and `unavailable`
        // are not failures either — telling the user to fix them is noise.
        let validation = parse_validation(&json!({
            "stages": [
                {"stage": "Full test suite", "outcome": "skipped"},
                {"stage": "Focused tests", "outcome": "unavailable"},
                {"stage": "Build", "outcome": "failed", "detail": "compile error"},
            ],
        }));
        let problems = problems_from(&validation);
        assert_eq!(problems.len(), 1);
        assert_eq!(problems[0].category, ProblemCategory::Build);
    }

    #[test]
    fn a_missing_tool_is_an_environment_problem_not_a_test_failure() {
        // "Install ruff" and "fix the failing test" are different actions.
        let validation = parse_validation(&json!({
            "stages": [{
                "stage": "Focused tests",
                "outcome": "failed",
                "detail": "sandbox-exec: execvp() of 'ruff' failed",
            }],
        }));
        let problems = problems_from(&validation);
        assert_eq!(problems[0].category, ProblemCategory::Environment);
    }

    #[test]
    fn sessions_from_another_folder_are_not_shown_in_this_one() {
        // Opening a different folder must not inherit the previous folder's
        // work: the titles look plausible, the branch is wrong, and selecting
        // one silently moves the user to another project.
        let here = std::env::temp_dir();
        let rows = vec![
            json!({"id": "a", "objective": "mine", "repository": here.to_string_lossy()}),
            json!({"id": "b", "objective": "someone else's", "repository": "/somewhere/else"}),
            json!({"id": "c", "objective": "unattributed"}),
        ];
        let kept = parse_session_rows_for(&rows, &here);
        assert_eq!(
            kept.len(),
            1,
            "got {:?}",
            kept.iter().map(|r| &r.id).collect::<Vec<_>>()
        );
        assert_eq!(kept[0].id, "a");
    }

    #[test]
    fn an_unreadable_environment_is_an_environment_problem_whatever_stage_hit_it() {
        // `$HOME` unset breaks cargo, npm and pytest alike. Filing it under the
        // stage that happened to trip over it sends the user to fix a test
        // that is not broken.
        for stage in ["Build", "Targeted tests", "Lint", "Formatting"] {
            let validation = parse_validation(&json!({
                "stages": [{
                    "stage": stage,
                    "outcome": "failed",
                    "detail": "cargo: error: Cargo couldn't find your home directory. This probably means that $HOME was not set.",
                }],
            }));
            let problems = problems_from(&validation);
            assert_eq!(
                problems[0].category,
                ProblemCategory::Environment,
                "{stage} reported an environment failure"
            );
        }
    }

    #[test]
    fn a_problem_that_names_a_file_offers_to_open_it() {
        let validation = parse_validation(&json!({
            "stages": [{
                "stage": "Build",
                "outcome": "failed",
                "detail": "error at backend/api/search.py:42:8 unexpected token",
            }],
        }));
        let problems = problems_from(&validation);
        assert_eq!(
            problems[0].location,
            Some(("backend/api/search.py".to_owned(), 42))
        );
    }

    #[test]
    fn a_problem_with_no_location_does_not_invent_one() {
        let validation = parse_validation(&json!({
            "stages": [{"stage": "Build", "outcome": "failed", "detail": "it broke"}],
        }));
        assert_eq!(problems_from(&validation)[0].location, None);
    }

    #[test]
    fn memory_entries_flatten_out_of_their_kind_groups() {
        let parsed = MemoryEntry::parse_all(&json!({
            "entries": {
                "build": [{
                    "id": "m1",
                    "content": "cargo test --workspace",
                    "source": "README.md",
                    "confidence": "verified",
                    "scope": "repository",
                    "created_at": "2026-08-08T10:00:00Z",
                    "last_used_at": "2026-08-08T11:00:00Z",
                }],
                "learnings": [{
                    "id": "m2",
                    "content": "Integration tests need Redis",
                    "source": "Session \"Fix auth test\"",
                    "confidence": "unverified",
                    "scope": "repository",
                    "created_at": "2026-08-07T09:00:00Z",
                }],
            }
        }));
        assert_eq!(parsed.len(), 2);
        let build = parsed
            .iter()
            .find(|e| e.kind == "build")
            .expect("build entry");
        assert_eq!(build.content, "cargo test --workspace");
        let learning = parsed
            .iter()
            .find(|e| e.kind == "learnings")
            .expect("learning");
        // Nothing has drawn on it yet, which is worth being able to see.
        assert!(learning.last_used_at.is_none());
        assert!(learning.provenance().contains("never used"));
        assert!(build.provenance().contains("last used"));
    }

    #[test]
    fn a_memory_entry_with_no_id_is_dropped() {
        // Edit and forget both address an entry by id, so an entry without
        // one would render two buttons that cannot work.
        let parsed = MemoryEntry::parse_all(&json!({
            "entries": {"build": [{"content": "no id"}]}
        }));
        assert!(parsed.is_empty());
    }

    #[test]
    fn every_memory_entry_carries_its_provenance() {
        // The reason this surface exists: a fact with no visible source is a
        // fact nobody can check or correct.
        let parsed = MemoryEntry::parse_all(&json!({
            "entries": {"user_rules": [{
                "id": "m3",
                "content": "Never modify generated files",
                "source": "Added by you in Settings",
                "confidence": "unverified",
                "scope": "repository",
                "created_at": "2026-08-08T10:00:00Z",
            }]}
        }));
        let provenance = parsed[0].provenance();
        assert!(provenance.contains("Added by you in Settings"));
        assert!(provenance.contains("unverified"));
        assert!(provenance.contains("repository"));
    }

    #[test]
    fn an_unparseable_timestamp_is_shown_rather_than_hidden() {
        // A placeholder would hide a format problem; the raw value lets
        // somebody recognise it.
        assert_eq!(relative_time("not a date"), "not a date");
        assert_eq!(relative_time(""), "");
    }
}
