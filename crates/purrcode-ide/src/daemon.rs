//! Authenticated loopback client for the one PurrCode daemon.
//!
//! The IDE is a client, not a runtime (PRD §2.1). It owns no session store, no
//! model state and no execution path: every fact on screen came from `purrcoded`
//! and every action goes back to it as a typed request.
//!
//! egui repaints on the UI thread and must never block on a socket, so all HTTP
//! runs on a worker thread. The UI enqueues [`Request`]s and drains
//! [`Response`]s; a slow or dead daemon costs frames, not responsiveness.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::Duration;

use serde::de::DeserializeOwned;
use serde_json::Value;

use purrcode_terminal_runtime::{StartTerminalAction, TerminalSize};
use purrcode_workspace_contracts::WorkspaceId;

pub const QUERY_WORKERS: usize = 4;
pub const QUERY_QUEUE_CAPACITY: usize = 64;
pub const CONTROL_WORKERS: usize = 1;
pub const URGENT_CONTROL_WORKERS: usize = 1;

/// Monotonic client-side generation for a session snapshot load.
///
/// The daemon may answer an older load after a newer one (the query workers
/// intentionally run in parallel).  Tagging every load response with this
/// generation lets the UI ignore stale panels and completion signals without
/// keeping an unbounded request table in the dispatcher.
static NEXT_SESSION_LOAD_GENERATION: AtomicU64 = AtomicU64::new(1);

/// Where the daemon is and how to prove we may talk to it.
#[derive(Clone, Debug)]
pub struct Connection {
    pub base_url: String,
    pub token: String,
}

impl Connection {
    pub fn new(base_url: impl Into<String>, token: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into().trim_end_matches('/').to_owned(),
            token: token.into(),
        }
    }
}

/// Something the UI wants from the daemon.
///
/// Each variant names a product intent rather than an endpoint, so a route
/// change stays inside this module instead of spreading through the views.
#[derive(Clone, Debug)]
pub enum Request {
    Bootstrap,
    /// List the sessions belonging to one folder. Never all of them: another
    /// folder's sessions look plausible and open the wrong project.
    ListSessions {
        repository: String,
    },
    /// The folder's own branch and GitHub state, which exist before any
    /// session does.
    WorkspaceState {
        repository: String,
    },
    /// Per-file change counts for the open folder (the user's own checkout),
    /// fetched independently of any session worktree. Polled at the workspace
    /// cadence, not the session cadence, because numstat over a dirty tree is
    /// not free.
    WorkspaceChanges {
        repository: String,
        scope: &'static str,
    },
    /// Everything one session view needs, fetched together so the screen never
    /// shows a half-updated session.
    /// Load one session. `scope` selects which change set the snapshot's
    /// change counts describe, so the summary and the patch always agree.
    LoadSession {
        session: String,
        scope: &'static str,
    },
    /// Re-fetch one presentation panel without waiting for the other panels.
    /// A failed evidence request is independently retryable (PRD §5.2).
    RetryPanel {
        session: String,
        panel: PanelKind,
        scope: &'static str,
    },
    StartSession {
        objective: String,
        repository: String,
        model: Option<String>,
        task_mode: String,
        execution_style: String,
        permission_mode: String,
        plan_only: bool,
    },
    SendMessage {
        session: String,
        content: String,
    },
    SessionAction {
        session: String,
        action: &'static str,
        body: Value,
    },
    SetModel {
        session: String,
        model: String,
    },
    /// Add and probe a provider profile. The credential is only a credential
    /// name/reference; the IDE never receives or forwards the secret itself.
    ConfigureProvider {
        name: String,
        provider_type: String,
        base_url: String,
        model: String,
        credential_name: Option<String>,
        /// An inline API key (from the simplified form) stored to
        /// `credentials.toml` by the daemon before the profile is saved.
        secret: Option<String>,
        /// `true` when the named provider already exists: the daemon refuses a
        /// duplicate profile unless the edit is explicit (FR-A2).
        replace: bool,
    },
    /// Assign a configured model to a daemon role (for example,
    /// `coding_worker`) and refresh the model list for the settings surface.
    AssignModelRole {
        role: String,
        model: String,
    },
    SetControls {
        session: String,
        controls: Value,
    },
    FileDiff {
        session: String,
        /// Which change set to fetch: `agent`, `working_tree` or `staged`.
        scope: &'static str,
    },
    ReviewHunks {
        session: String,
    },
    ReviewHunk {
        session: String,
        action: &'static str,
        index: usize,
        patch_digest: String,
    },
    ListModels,
    StartTerminal {
        /// Where the shell should start. This is the session's worktree when
        /// there is one, so the terminal sees the tree the agent changed.
        working_directory: String,
        rows: u16,
        cols: u16,
    },
    PollTerminal {
        terminal: String,
        since: u64,
    },
    SendTerminalInput {
        terminal: String,
        generation: u64,
        /// Already encoded for the terminal's current modes, so the daemon
        /// writes them to the PTY untouched.
        bytes: Vec<u8>,
    },
    ResizeTerminal {
        terminal: String,
        rows: u16,
        cols: u16,
    },
    StopTerminal {
        terminal: String,
    },
    /// Hand a terminal to the human, or back to an agent (PRD §16, §17).
    SetTerminalOwner {
        terminal: String,
        /// `None` means the human takes over.
        agent_role: Option<String>,
    },
    /// Re-discover terminals the daemon still has after a GUI restart
    /// (PRD §41 reconnect).
    ListTerminals,
    // ── Settings surfaces (Defect A) ─────────────────────────────────
    /// `GET /v1/providers` — the configured provider profile names.
    ListProviders,
    /// `GET /v1/providers/{name}` — one provider's configuration and models.
    GetProvider {
        name: String,
    },
    /// `POST /v1/providers/test` — run the connection probe for one provider.
    TestProvider {
        name: String,
    },
    /// `DELETE /v1/providers/{name}` — remove a provider profile.
    RemoveProvider {
        name: String,
    },
    /// `POST /v1/providers/discover` — list models a local runtime reports.
    DiscoverProviderModels {
        provider_type: String,
    },
    /// `GET /v1/local-models` — reachability, installed/loaded models, memory.
    LocalModelsStatus,
    /// `GET /v1/local-models/recommendations` — qualification cards + risks.
    LocalModelsRecommendations,
    /// `POST /v1/local-models/qualify` — qualifies an installed model. This is
    /// the one multi-minute call, so it runs on a dedicated long-timeout client.
    LocalModelsQualify {
        model: String,
    },
    /// `POST /v1/local-models/unload` — unload a loaded local model.
    LocalModelsUnload {
        model: String,
        all: bool,
    },
    /// `POST /v1/local-models/pull/propose` — step one of the pull gate.
    LocalModelsPullPropose {
        session_id: Option<String>,
        repository: Option<String>,
        model: String,
    },
    /// `POST /v1/local-models/pull/{id}/approve` — step two.
    LocalModelsPullApprove {
        session_id: String,
        action_id: String,
    },
    /// `POST /v1/local-models/pull/{id}/start` — step three.
    LocalModelsPullStart {
        session_id: String,
        action_id: String,
    },
    /// `GET /v1/local-models/pull/{id}` — poll the JSON progress route.
    LocalModelsPullPoll {
        action_id: String,
    },
    /// `POST /v1/local-models/pull/{id}/cancel` — available throughout.
    LocalModelsPullCancel {
        session_id: String,
        action_id: String,
    },
    /// `GET /v1/local-models/settings` — the lifecycle policy.
    LocalModelsGetSettings,
    /// `POST /v1/local-models/settings` — save the lifecycle policy.
    LocalModelsPutSettings {
        settings: Value,
    },
    /// `GET /v1/skills` — the installed skill records.
    ListSkills,
    /// `GET /v1/skills/{id}` — one installed skill record.
    GetSkill {
        id: String,
    },
    /// `DELETE /v1/skills/{id}` — remove an installed skill.
    RemoveSkill {
        id: String,
    },
    /// `POST /v1/skills/search` — the approval-gated registry search.
    SkillSearch {
        session_id: String,
        capability: String,
        keywords: Vec<String>,
        action_id: Option<String>,
    },
    /// `POST /v1/skills/download` — download an inspected candidate.
    SkillDownload {
        session_id: String,
        candidate_id: String,
        commit: String,
        action_id: Option<String>,
    },
    /// `POST /v1/skills/install/propose` — qualify and propose an install.
    SkillInstallPropose {
        session_id: String,
        candidate_id: String,
        version: String,
        scope: String,
        source_path: String,
        content_digest: String,
        publisher: Option<String>,
        approved_permissions: Value,
        signature: Option<String>,
        publisher_public_key: Option<String>,
    },
    /// `POST /v1/skills/install/{id}/approve` — approve a proposed install.
    SkillInstallApprove {
        session_id: String,
        action_id: String,
    },
    /// `POST /v1/skills/install` — execute an approved install.
    SkillInstall {
        session_id: String,
        action_id: String,
    },
    /// `POST /v1/skills/publishers/block` — block a skill publisher.
    SkillBlockPublisher {
        publisher: String,
        reason: String,
    },
    /// `GET /v1/mcp/servers` — the configured MCP servers.
    McpList,
    /// `POST /v1/mcp/servers` — add or replace one server.
    McpUpsert {
        server: Value,
    },
    /// `DELETE /v1/mcp/servers/{id}` — remove one server.
    McpRemove {
        id: String,
    },
    /// `POST /v1/sessions/{id}/mcp` with `tool: "__discover__"` — probe one
    /// server's tools for the selected session.
    McpProbe {
        session: String,
        server: String,
    },
    /// `GET /v1/codex` — the current `[codex]` table.
    CodexGet,
    /// `POST /v1/codex` — persist the `[codex]` table.
    CodexPut {
        config: Value,
    },
    /// `POST /v1/codex/doctor` — run `CodexBridge::doctor`.
    CodexDoctor,
    // ── Language intelligence ────────────────────────────────────────
    //
    // These are reads, so they stay on the query lane rather than the serial
    // control lane: a hover must never queue behind a running agent turn.
    /// `GET /v1/lsp/servers` — which language servers this machine has.
    LspServers,
    /// `POST /v1/lsp/open` — hand a document to its server so it starts
    /// analysing. Sent on open and after a save.
    LspOpen {
        path: PathBuf,
        root: PathBuf,
        text: String,
    },
    /// `POST /v1/lsp/hover` — the type/doc text at a position.
    LspHover {
        path: PathBuf,
        root: PathBuf,
        line: u64,
        character: u64,
    },
    /// `POST /v1/lsp/definition` — where the symbol at a position is defined.
    LspDefinition {
        path: PathBuf,
        root: PathBuf,
        line: u64,
        character: u64,
    },
    /// `POST /v1/lsp/references` — every use of the symbol at a position.
    LspReferences {
        path: PathBuf,
        root: PathBuf,
        line: u64,
        character: u64,
        /// The identifier the user asked about, echoed back so the results
        /// panel can name what it is listing.
        label: String,
    },
    /// `POST /v1/lsp/symbols` — the document outline.
    LspSymbols {
        path: PathBuf,
        root: PathBuf,
    },
    /// `POST /v1/lsp/format` — whole-document formatting edits.
    LspFormat {
        path: PathBuf,
        root: PathBuf,
        /// `true` when the format was triggered by a save, so the editor
        /// writes the formatted text back to disk once the edits land.
        then_save: bool,
    },
    /// `GET /v1/lsp/diagnostics` — everything the servers have published.
    LspDiagnostics,
}

/// A panel in the session presentation snapshot.
///
/// The value and its availability are deliberately separate. An empty array
/// is a successful check that found nothing; a missing value with an error is
/// a failed check and must never be rendered as an empty panel.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum PanelKind {
    Summary,
    Conversation,
    Activity,
    Artifacts,
    Changes,
    Validation,
    Usage,
    Controls,
    Github,
    Spec,
    Tasks,
    Evidence,
}

impl PanelKind {
    pub const ALL: &'static [Self] = &[
        Self::Summary,
        Self::Conversation,
        Self::Activity,
        Self::Artifacts,
        Self::Changes,
        Self::Validation,
        Self::Usage,
        Self::Controls,
        Self::Github,
        Self::Spec,
        Self::Tasks,
        Self::Evidence,
    ];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Summary => "Summary",
            Self::Conversation => "Conversation",
            Self::Activity => "Activity",
            Self::Artifacts => "Artifacts",
            Self::Changes => "Changes",
            Self::Validation => "Validation",
            Self::Usage => "Usage",
            Self::Controls => "Controls",
            Self::Github => "GitHub",
            Self::Spec => "Spec",
            Self::Tasks => "Tasks",
            Self::Evidence => "Evidence",
        }
    }
}

/// The provenance of one daemon presentation request.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PanelAvailability {
    /// The request has not completed yet (used by the UI while a retry is in
    /// flight). A `LoadSession` response normally replaces this immediately.
    Loading,
    /// The daemon answered successfully with meaningful data.
    Ready,
    /// The daemon answered successfully and the result is empty.
    Empty,
    /// The panel is not available for this session (for example, no worktree).
    Unavailable,
    /// The request failed. The detail remains available for an honest error
    /// message and diagnostics instead of becoming `null`.
    Error,
}

/// One typed panel result, retaining status, error provenance, and freshness.
#[derive(Clone, Debug)]
pub struct PanelResult {
    pub availability: PanelAvailability,
    pub value: Option<Value>,
    pub error: Option<String>,
    /// RFC3339 UTC timestamp, suitable for a "last checked" label without
    /// making the UI infer freshness from a local clock.
    pub fetched_at: Option<String>,
}

impl Default for PanelResult {
    fn default() -> Self {
        Self {
            availability: PanelAvailability::Loading,
            value: None,
            error: None,
            fetched_at: None,
        }
    }
}

impl PanelResult {
    pub fn loading() -> Self {
        Self::default()
    }

    fn now() -> String {
        chrono::Utc::now().to_rfc3339()
    }

    pub fn success(value: Value) -> Self {
        let typed_state = value.get("state").and_then(Value::as_str);
        let availability = match typed_state {
            Some("loading") => PanelAvailability::Loading,
            Some("ready") => PanelAvailability::Ready,
            Some("empty") => PanelAvailability::Empty,
            Some("unavailable") => PanelAvailability::Unavailable,
            Some("error") => PanelAvailability::Error,
            _ if value
                .get("status")
                .and_then(Value::as_str)
                .is_some_and(|status| status.eq_ignore_ascii_case("unavailable")) =>
            {
                PanelAvailability::Unavailable
            }
            _ if value.is_null() || value.as_array().is_some_and(|values| values.is_empty()) => {
                PanelAvailability::Empty
            }
            _ => PanelAvailability::Ready,
        };
        let error = (availability == PanelAvailability::Error).then(|| {
            value
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("daemon panel reported an error")
                .to_owned()
        });
        let fetched_at = value
            .get("observed_at")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned)
            .or_else(|| Some(Self::now()));
        Self {
            availability,
            value: Some(value),
            error,
            fetched_at,
        }
    }

    pub fn failure(error: String) -> Self {
        // A missing route is a capability/version gap, not an empty result.
        // Keep the distinction visible so a client can say "unavailable" and
        // offer Retry without claiming the daemon checked and found nothing.
        let availability = if error.contains(" 404:") || error.contains(" returned 404") {
            PanelAvailability::Unavailable
        } else {
            PanelAvailability::Error
        };
        Self {
            availability,
            value: None,
            error: Some(error),
            fetched_at: Some(Self::now()),
        }
    }

    pub fn value_or_null(&self) -> Value {
        self.value.clone().unwrap_or(Value::Null)
    }
}

/// A snapshot of one session, assembled from the presentation endpoints.
#[derive(Clone, Debug, Default)]
pub struct SessionSnapshot {
    pub summary: Value,
    pub conversation: Vec<Value>,
    pub activity: Vec<Value>,
    pub artifacts: Vec<Value>,
    pub changes: Value,
    pub validation: Value,
    pub usage: Value,
    pub controls: Value,
    pub github: Value,
    pub spec: Value,
    pub tasks: Value,
    pub evidence: Value,
    /// Provenance for every value above. The legacy value fields remain for
    /// defensive parsers, but callers must use this map for availability.
    pub panels: BTreeMap<PanelKind, PanelResult>,
}

impl SessionSnapshot {
    pub fn loading() -> Self {
        let panels = PanelKind::ALL
            .iter()
            .copied()
            .map(|kind| (kind, PanelResult::loading()))
            .collect();
        Self {
            panels,
            ..Self::default()
        }
    }

    pub fn panel(&self, kind: PanelKind) -> PanelResult {
        self.panels.get(&kind).cloned().unwrap_or_default()
    }

    pub(crate) fn set_panel(&mut self, kind: PanelKind, result: PanelResult) {
        let value = result.value_or_null();
        match kind {
            PanelKind::Summary => self.summary = value,
            PanelKind::Conversation => {
                self.conversation = value.as_array().cloned().unwrap_or_default()
            }
            PanelKind::Activity => self.activity = value.as_array().cloned().unwrap_or_default(),
            PanelKind::Artifacts => self.artifacts = value.as_array().cloned().unwrap_or_default(),
            PanelKind::Changes => self.changes = value,
            PanelKind::Validation => self.validation = value,
            PanelKind::Usage => self.usage = value,
            PanelKind::Controls => self.controls = value,
            PanelKind::Github => self.github = value,
            PanelKind::Spec => self.spec = value,
            PanelKind::Tasks => self.tasks = value,
            PanelKind::Evidence => self.evidence = value,
        }
        self.panels.insert(kind, result);
    }
}

/// Something the daemon said back.
#[derive(Clone, Debug)]
pub enum Response {
    Bootstrap(Value),
    Sessions(Vec<Value>),
    Session(String, Box<SessionSnapshot>),
    /// The loading snapshot and its generation. A later [`SessionLoaded`]
    /// carries the same generation after all panel jobs have completed.
    SessionLoading(String, u64, Box<SessionSnapshot>),
    /// One panel completed independently of the rest of a snapshot.
    Panel(String, PanelKind, PanelResult),
    /// One panel belonging to a [`SessionLoading`] generation completed. This
    /// is separate from `Panel`, which is retained for an uncorrelated manual
    /// `RetryPanel` request.
    SessionPanel(String, u64, PanelKind, PanelResult),
    /// Every panel job for the generation has produced a result (including a
    /// bounded-queue error). The UI may clear its loading state immediately;
    /// it must compare the generation before doing so.
    SessionLoaded(String, u64),
    /// A session was created; the UI should select it.
    SessionStarted(String),
    /// A mutation completed and the named session should be reloaded.
    Mutated(String),
    Diff(String, String),
    Hunks(String, Value),
    Models(Vec<Value>),
    TerminalStarted(Value),
    TerminalOutput(Value),
    /// One terminal's metadata changed (ownership, liveness).
    TerminalChanged(Value),
    /// Every terminal the daemon is holding.
    Terminals(Vec<Value>),
    /// The open folder's branch and GitHub state.
    Workspace(Value),
    /// Per-file change counts for the open folder, parsed from the
    /// workspace-changes route (the user's own checkout, not a session
    /// worktree).
    WorkspaceChanges(Value),
    // ── Settings surfaces (Defect A) ─────────────────────────────────
    /// `GET /v1/providers` — the provider profile names.
    Providers(Vec<Value>),
    /// `GET /v1/providers/{name}` — one provider's config and models.
    Provider(Value),
    /// `POST /v1/providers/test` — one provider's probe result.
    ProviderTested(Value),
    /// `POST /v1/providers/discover` — models a local runtime reports.
    DiscoveredModels(Value),
    /// `GET /v1/local-models` — reachability, installed/loaded, memory.
    LocalModels(Value),
    /// `GET /v1/local-models/recommendations` — qualification cards + risks.
    LocalModelsRecommendations(Value),
    /// `POST /v1/local-models/qualify` — a qualification report.
    LocalModelsQualified(Value),
    /// `POST /v1/local-models/unload` — the unload result.
    LocalModelsUnloaded(Value),
    /// `POST /v1/local-models/pull/propose` — the pull approval proposal.
    LocalModelsPullProposed(Value),
    /// `POST /v1/local-models/pull/{id}/approve`.
    LocalModelsPullApproved(Value),
    /// `POST /v1/local-models/pull/{id}/start` — a `PullProgress`.
    LocalModelsPullStarted(Value),
    /// `GET /v1/local-models/pull/{id}` — a `PullProgress` poll snapshot.
    LocalModelsPullProgress(Value),
    /// `POST /v1/local-models/pull/{id}/cancel`.
    LocalModelsPullCancelled(Value),
    /// `GET|POST /v1/local-models/settings` — the lifecycle policy.
    LocalModelsSettings(Value),
    /// `GET /v1/skills` — installed skill records.
    Skills(Vec<Value>),
    /// `GET /v1/skills/{id}` — one skill record.
    Skill(Value),
    /// `DELETE /v1/skills/{id}` — the removed record.
    SkillRemoved(Value),
    /// `POST /v1/skills/search` — the approval-gated search answer.
    SkillSearch(Value),
    /// `POST /v1/skills/download` — the downloaded candidate.
    SkillDownloaded(Value),
    /// `POST /v1/skills/install/propose` — the proposal.
    SkillInstallProposed(Value),
    /// `POST /v1/skills/install/{id}/approve`.
    SkillInstallApproved(Value),
    /// `POST /v1/skills/install` — the installed record.
    SkillInstalled(Value),
    /// `POST /v1/skills/publishers/block`.
    SkillPublisherBlocked(Value),
    /// `GET /v1/mcp/servers` — the configured servers (a JSON object map).
    McpServers(Value),
    /// `POST /v1/mcp/servers` — the saved server.
    McpServerSaved(Value),
    /// `DELETE /v1/mcp/servers/{id}`.
    McpServerRemoved(Value),
    /// `POST /v1/sessions/{id}/mcp` with `tool: "__discover__"`.
    McpProbed(Value),
    /// `GET /v1/codex` — the current `[codex]` table.
    Codex(Value),
    /// `POST /v1/codex` — the persisted table.
    CodexSaved(Value),
    /// `POST /v1/codex/doctor` — a `CodexDoctorReport`.
    CodexDoctor(Value),
    // ── Language intelligence ────────────────────────────────────────
    /// `GET /v1/lsp/servers` — the servers available on this machine.
    LspServers(Value),
    /// `POST /v1/lsp/hover`, correlated with the document and position that
    /// asked. The UI drops a reply whose anchor the pointer has already left
    /// rather than showing one token's type against another.
    LspHover(PathBuf, u64, u64, Value),
    /// `POST /v1/lsp/definition` — the target locations.
    LspDefinition(Value),
    /// `POST /v1/lsp/references` — the symbol that was asked about, and its uses.
    LspReferences(String, Value),
    /// `POST /v1/lsp/symbols` — the outline of one document.
    LspSymbols(PathBuf, Value),
    /// `POST /v1/lsp/format` — the edits, the document, and whether the editor
    /// should save once they are applied.
    LspFormat(PathBuf, Value, bool),
    /// `GET /v1/lsp/diagnostics` — every published diagnostic.
    LspDiagnostics(Value),
    /// A language-server request failed. Separate from the generic failure
    /// path so a missing rust-analyzer explains itself in the editor instead
    /// of raising a modal notice on every keystroke.
    LspUnavailable(String),
    /// A settings mutation landed; the UI refetches the affected page.
    SettingsMutated,
    /// Connectivity changed. `false` means every view should say so rather than
    /// keep rendering stale data as if it were live.
    Connected(bool),
    /// A start/follow-up message failed. Kept separate from background query
    /// failures so the UI restores only the draft that actually failed.
    SubmissionFailed(String),
    Failed(String),
}

/// The UI-side handle: enqueue requests, drain responses, never block.
pub struct DaemonClient {
    query_outbound: Sender<Request>,
    control_outbound: Sender<Request>,
    urgent_outbound: Sender<Request>,
    inbound: Receiver<Response>,
    local_responses: Sender<Response>,
    repaint: Arc<dyn Fn() + Send + Sync>,
    connection: Connection,
}

impl DaemonClient {
    pub fn spawn(connection: Connection, repaint: impl Fn() + Send + Sync + 'static) -> Self {
        let (query_outbound, query_requests) = mpsc::channel::<Request>();
        let (control_outbound, control_requests) = mpsc::channel::<Request>();
        let (urgent_outbound, urgent_requests) = mpsc::channel::<Request>();
        let (responses, inbound) = mpsc::channel::<Response>();
        let worker_connection = connection.clone();
        let repaint: Arc<dyn Fn() + Send + Sync> = Arc::new(repaint);
        let http = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(30))
            // The daemon is on loopback; a proxy would both break the
            // connection and leak the request to whatever it points at.
            .no_proxy()
            // Do not hold idle connections open.
            //
            // The client polls continuously, so it accumulates pooled
            // keep-alive connections. The daemon closes them on its own idle
            // timeout, and the next request to pick a dead one fails with a
            // transport error that is neither a connect failure nor a timeout —
            // it surfaced to users as the raw text "error sending request for
            // url (http://127.0.0.1:7377/v1/sessions)" while the daemon was
            // healthy and answering curl in four milliseconds.
            //
            // On loopback a fresh connection costs microseconds, so paying for
            // one per request is cheaper than the class of bug it removes.
            .pool_max_idle_per_host(0)
            .build()
            .expect("build blocking HTTP client");
        let worker = Worker {
            http,
            connection: worker_connection,
            responses: responses.clone(),
            repaint: repaint.clone(),
        };

        // Query work is bounded: four workers share a finite queue. A slow
        // snapshot can therefore consume at most four blocking sockets and
        // cannot create one OS thread per poll/session.
        let (query_jobs, query_queue) = mpsc::sync_channel::<QueryJob>(QUERY_QUEUE_CAPACITY);
        let query_queue = Arc::new(std::sync::Mutex::new(query_queue));
        for index in 0..QUERY_WORKERS {
            let worker = worker.clone();
            let queue = Arc::clone(&query_queue);
            std::thread::Builder::new()
                .name(format!("purrcode-ide-query-{index}"))
                .spawn(move || {
                    loop {
                        let job = match queue.lock().expect("query queue lock").recv() {
                            Ok(job) => job,
                            Err(_) => break,
                        };
                        worker.handle_query(job);
                        (worker.repaint)();
                    }
                })
                .expect("spawn bounded IDE query worker");
        }

        // Urgent safety controls have a dedicated worker, separate from the
        // ordinary mutation worker. A slow StartSession/SendMessage request
        // therefore cannot hold Stop/Approve/Reject or human terminal input.
        let urgent_worker = worker.clone();
        std::thread::Builder::new()
            .name("purrcode-ide-urgent-control".into())
            .spawn(move || {
                while let Ok(request) = urgent_requests.recv() {
                    urgent_worker.handle(request);
                    (urgent_worker.repaint)();
                }
            })
            .expect("spawn IDE urgent control worker");

        let control_worker = worker.clone();
        std::thread::Builder::new()
            .name("purrcode-ide-control".into())
            .spawn(move || {
                while let Ok(request) = control_requests.recv() {
                    control_worker.handle(request);
                    (control_worker.repaint)();
                }
            })
            .expect("spawn IDE control worker");

        // The dispatcher performs no blocking I/O. LoadSession emits a typed
        // Loading snapshot and a generation, then enqueues twelve bounded
        // panel jobs; any full queue becomes an explicit panel error with
        // Retry, never a dropped request or an unbounded thread burst.
        let dispatcher_worker = worker.clone();
        std::thread::Builder::new()
            .name("purrcode-ide-query-dispatch".into())
            .spawn(move || {
                while let Ok(request) = query_requests.recv() {
                    if let Err(request) = dispatch_query(request, &dispatcher_worker, &query_jobs) {
                        dispatcher_worker.reply(Response::Failed(request));
                        (dispatcher_worker.repaint)();
                    }
                }
            })
            .expect("spawn IDE query dispatcher");
        Self {
            query_outbound,
            control_outbound,
            urgent_outbound,
            inbound,
            local_responses: responses,
            repaint,
            connection,
        }
    }

    pub fn connection(&self) -> &Connection {
        &self.connection
    }

    pub fn send(&self, request: Request) {
        // A worker can fail independently of the egui event loop. Never turn
        // a visible click into a silently dropped action: surface the broken
        // local queue through the same response path as HTTP failures.
        let result = if request.is_urgent_control() {
            self.urgent_outbound.send(request)
        } else if request.is_control() {
            self.control_outbound.send(request)
        } else {
            self.query_outbound.send(request)
        };
        if let Err(error) = result {
            let response = match error.0 {
                Request::StartSession { .. } | Request::SendMessage { .. } => {
                    Response::SubmissionFailed(
                        "PurrCode's local control worker stopped before the message was sent"
                            .into(),
                    )
                }
                _ => Response::Failed(
                    "PurrCode's local control worker stopped before the action was sent".into(),
                ),
            };
            let _ = self.local_responses.send(response);
            (self.repaint)();
        }
    }

    /// Every response that has arrived since the last frame.
    pub fn drain(&self) -> Vec<Response> {
        let mut received = Vec::new();
        while let Ok(response) = self.inbound.try_recv() {
            received.push(response);
        }
        received
    }
}

/// Work accepted by the bounded query pool. Panel fetches are separate jobs so
/// one slow endpoint cannot hold the other eight panels hostage.
enum QueryJob {
    Request(Request),
    Panel {
        session: String,
        generation: u64,
        panel: PanelKind,
        path: String,
        load: Arc<SessionLoadState>,
    },
    RetryPanel {
        session: String,
        panel: PanelKind,
        path: String,
    },
}

impl Request {
    fn is_urgent_control(&self) -> bool {
        match self {
            Self::SessionAction { action, .. } => {
                matches!(*action, "cancel" | "approve" | "reject")
            }
            Self::SendTerminalInput { .. }
            | Self::StopTerminal { .. }
            | Self::SetTerminalOwner { .. } => true,
            _ => false,
        }
    }

    fn is_control(&self) -> bool {
        self.is_urgent_control()
            || matches!(
                self,
                Self::StartSession { .. }
                    | Self::SendMessage { .. }
                    | Self::SessionAction { .. }
                    | Self::SetModel { .. }
                    | Self::ConfigureProvider { .. }
                    | Self::AssignModelRole { .. }
                    | Self::SetControls { .. }
                    | Self::ReviewHunk { .. }
                    | Self::StartTerminal { .. }
                    | Self::ResizeTerminal { .. }
                    // Settings mutations: the provider, local-model, skill,
                    // MCP and Codex writes are durable daemon state, so they
                    // share the serial control lane with the other mutations.
                    | Self::TestProvider { .. }
                    | Self::RemoveProvider { .. }
                    | Self::DiscoverProviderModels { .. }
                    | Self::LocalModelsQualify { .. }
                    | Self::LocalModelsUnload { .. }
                    | Self::LocalModelsPullPropose { .. }
                    | Self::LocalModelsPullApprove { .. }
                    | Self::LocalModelsPullStart { .. }
                    | Self::LocalModelsPullCancel { .. }
                    | Self::LocalModelsPutSettings { .. }
                    | Self::RemoveSkill { .. }
                    | Self::SkillSearch { .. }
                    | Self::SkillDownload { .. }
                    | Self::SkillInstallPropose { .. }
                    | Self::SkillInstallApprove { .. }
                    | Self::SkillInstall { .. }
                    | Self::SkillBlockPublisher { .. }
                    | Self::McpUpsert { .. }
                    | Self::McpRemove { .. }
                    | Self::McpProbe { .. }
                    | Self::CodexPut { .. }
                    | Self::CodexDoctor
            )
    }
}

/// Shared accounting for one bounded session snapshot load.
///
/// A load has exactly one slot per panel. A slot is completed either by a
/// query worker after its HTTP request, or immediately by the dispatcher when
/// the bounded queue is full. The final slot emits one `SessionLoaded` event.
/// Keeping this state in an `Arc` attached to each job avoids a dispatcher map
/// that could grow with every historical-session poll.
struct SessionLoadState {
    session: String,
    generation: u64,
    remaining: AtomicUsize,
}

impl SessionLoadState {
    fn new(session: String, generation: u64) -> Arc<Self> {
        Arc::new(Self {
            session,
            generation,
            remaining: AtomicUsize::new(PanelKind::ALL.len()),
        })
    }
}

fn enqueue_panel(
    jobs: &mpsc::SyncSender<QueryJob>,
    worker: &Worker,
    session: String,
    generation: u64,
    panel: PanelKind,
    scope: &'static str,
    load: &Arc<SessionLoadState>,
) {
    let path = panel_path(panel, &session, scope);
    if jobs
        .try_send(QueryJob::Panel {
            session: session.clone(),
            generation,
            panel,
            path,
            load: Arc::clone(load),
        })
        .is_err()
    {
        worker.reply(Response::SessionPanel(
            session,
            generation,
            panel,
            PanelResult::failure("IDE query queue is full; retry this panel".into()),
        ));
        worker.finish_session_load(load);
    }
}

fn enqueue_retry_panel(
    jobs: &mpsc::SyncSender<QueryJob>,
    worker: &Worker,
    session: String,
    panel: PanelKind,
    scope: &'static str,
) {
    let path = panel_path(panel, &session, scope);
    if jobs
        .try_send(QueryJob::RetryPanel {
            session: session.clone(),
            panel,
            path,
        })
        .is_ok()
    {
        return;
    }
    worker.reply(Response::Panel(
        session,
        panel,
        PanelResult::failure("IDE query queue is full; retry this panel".into()),
    ));
}

fn dispatch_query(
    request: Request,
    worker: &Worker,
    jobs: &mpsc::SyncSender<QueryJob>,
) -> Result<(), String> {
    match request {
        Request::LoadSession { session, scope } => {
            let generation = NEXT_SESSION_LOAD_GENERATION.fetch_add(1, Ordering::Relaxed);
            let load = SessionLoadState::new(session.clone(), generation);
            worker.reply(Response::SessionLoading(
                session.clone(),
                generation,
                Box::new(SessionSnapshot::loading()),
            ));
            (worker.repaint)();
            for panel in PanelKind::ALL {
                enqueue_panel(
                    jobs,
                    worker,
                    session.clone(),
                    generation,
                    *panel,
                    scope,
                    &load,
                );
            }
            (worker.repaint)();
            Ok(())
        }
        Request::RetryPanel {
            session,
            panel,
            scope,
        } => {
            worker.reply(Response::Panel(
                session.clone(),
                panel,
                PanelResult::loading(),
            ));
            (worker.repaint)();
            enqueue_retry_panel(jobs, worker, session, panel, scope);
            Ok(())
        }
        request => jobs
            .try_send(QueryJob::Request(request))
            .map_err(|_| "IDE query queue is full; retry the request".to_owned()),
    }
}

#[derive(Clone)]
struct Worker {
    http: reqwest::blocking::Client,
    connection: Connection,
    responses: Sender<Response>,
    repaint: Arc<dyn Fn() + Send + Sync>,
}

impl Worker {
    fn reply(&self, response: Response) {
        let _ = self.responses.send(response);
    }

    /// Mark one panel slot complete and emit exactly one generation-tagged
    /// completion event when the last slot is done.
    fn finish_session_load(&self, load: &SessionLoadState) {
        let remaining = load.remaining.fetch_sub(1, Ordering::AcqRel);
        debug_assert!(remaining > 0, "session load panel completed twice");
        if remaining == 1 {
            self.reply(Response::SessionLoaded(
                load.session.clone(),
                load.generation,
            ));
            (self.repaint)();
        }
    }

    /// Report transport health and the actionable error together. A bootstrap
    /// that once succeeded cannot leave the window green after the socket used
    /// for a mutation has demonstrably failed.
    fn reply_failure(&self, error: String) {
        if is_transport_failure(&error) {
            self.reply(Response::Connected(false));
        }
        self.reply(Response::Failed(error));
    }

    fn reply_submission_failure(&self, error: String) {
        if is_transport_failure(&error) {
            self.reply(Response::Connected(false));
        }
        self.reply(Response::SubmissionFailed(error));
    }

    fn handle_query(&self, job: QueryJob) {
        match job {
            QueryJob::Request(request) => self.handle(request),
            QueryJob::Panel {
                session,
                generation,
                panel,
                path,
                load,
            } => {
                let result = self.fetch_panel(panel, &path, &mut BTreeMap::new());
                self.reply(Response::SessionPanel(session, generation, panel, result));
                self.finish_session_load(&load);
            }
            QueryJob::RetryPanel {
                session,
                panel,
                path,
            } => {
                let result = self.fetch_panel(panel, &path, &mut BTreeMap::new());
                self.reply(Response::Panel(session, panel, result));
            }
        }
    }

    fn list_models(&self) {
        match self.get::<Value>("/v1/models") {
            Ok(value) => {
                let models = value
                    .as_array()
                    .cloned()
                    .or_else(|| value.get("models").and_then(Value::as_array).cloned())
                    .or_else(|| value.get("data").and_then(Value::as_array).cloned())
                    .or_else(|| value.get("items").and_then(Value::as_array).cloned())
                    .or_else(|| value.get("results").and_then(Value::as_array).cloned())
                    .unwrap_or_default();
                self.reply(Response::Models(models));
            }
            Err(error) => self.reply_failure(error),
        }
    }

    fn handle(&self, request: Request) {
        match request {
            Request::Bootstrap => match self.get::<Value>("/v1/bootstrap") {
                Ok(value) => {
                    self.reply(Response::Connected(true));
                    self.reply(Response::Bootstrap(value));
                }
                // Bootstrap is the connectivity probe: its failure is the one
                // that should flip the whole UI to "disconnected".
                Err(error) => {
                    self.reply(Response::Connected(false));
                    self.reply(Response::Failed(error));
                }
            },
            Request::ListSessions { repository } => {
                let query = urlencode(&repository);
                match self.get::<Vec<Value>>(&format!("/v1/sessions?repository={query}")) {
                    Ok(sessions) => self.reply(Response::Sessions(
                        sessions
                            .into_iter()
                            .filter(|session| !is_quarantined_legacy_session(session))
                            .collect(),
                    )),
                    Err(error) => self.reply_failure(error),
                }
            }
            Request::WorkspaceState { repository } => {
                let query = urlencode(&repository);
                match self.get::<Value>(&format!("/v1/workspace?repository={query}")) {
                    Ok(value) => self.reply(Response::Workspace(value)),
                    Err(error) => self.reply_failure(error),
                }
            }
            Request::WorkspaceChanges { repository, scope } => {
                let query = urlencode(&repository);
                match self.get::<Value>(&format!(
                    "/v1/workspace/changes?repository={query}&scope={scope}"
                )) {
                    Ok(value) => self.reply(Response::WorkspaceChanges(value)),
                    Err(error) => self.reply_failure(error),
                }
            }
            // The dispatcher expands these before they reach a query worker.
            // Keep explicit fallback responses so a future caller cannot
            // accidentally reintroduce an unbounded transport path.
            Request::LoadSession { .. } | Request::RetryPanel { .. } => self.reply(
                Response::Failed("session panel requests must use the bounded dispatcher".into()),
            ),
            Request::StartSession {
                objective,
                repository,
                model,
                task_mode,
                execution_style,
                permission_mode,
                plan_only,
            } => {
                let body = serde_json::json!({
                    "objective": objective,
                    "repository": repository,
                    "model": model,
                    "task_mode": task_mode,
                    "execution_style": execution_style,
                    "permission_mode": permission_mode,
                    "plan_only": plan_only,
                });
                match self.post::<Value>("/v1/sessions", &body) {
                    Ok(value) => match value.get("id").and_then(Value::as_str) {
                        Some(id) => self.reply(Response::SessionStarted(id.to_owned())),
                        None => self.reply(Response::SubmissionFailed(
                            "daemon accepted the session but returned no id".into(),
                        )),
                    },
                    Err(error) => self.reply_submission_failure(error),
                }
            }
            Request::SendMessage { session, content } => {
                let body = serde_json::json!({ "content": content });
                match self.post::<Value>(&format!("/v1/sessions/{session}/messages"), &body) {
                    Ok(_) => self.reply(Response::Mutated(session)),
                    Err(error) => self.reply_submission_failure(error),
                }
            }
            Request::SessionAction {
                session,
                action,
                body,
            } => match self.post::<Value>(&format!("/v1/sessions/{session}/{action}"), &body) {
                Ok(_) => self.reply(Response::Mutated(session)),
                Err(error) => self.reply_failure(error),
            },
            Request::SetModel { session, model } => {
                let body = serde_json::json!({ "model": model });
                match self.post::<Value>(&format!("/v1/sessions/{session}/model"), &body) {
                    Ok(_) => self.reply(Response::Mutated(session)),
                    Err(error) => self.reply_failure(error),
                }
            }
            Request::ConfigureProvider {
                name,
                provider_type,
                base_url,
                model,
                credential_name,
                secret,
                replace,
            } => {
                let body = provider_configuration_body(
                    name,
                    provider_type,
                    base_url,
                    model,
                    credential_name,
                    secret,
                    replace,
                );
                match self.post::<Value>("/v1/providers", &body) {
                    Ok(_) => {
                        self.list_models();
                        // Refresh the settings provider list so a newly added
                        // profile shows up without reopening the window.
                        self.reply(Response::SettingsMutated);
                    }
                    Err(error) => self.reply_failure(error),
                }
            }
            Request::AssignModelRole { role, model } => {
                let body = model_role_body(role, model);
                match self.post::<Value>("/v1/models/roles", &body) {
                    Ok(_) => self.list_models(),
                    Err(error) => self.reply_failure(error),
                }
            }
            Request::SetControls { session, controls } => {
                match self.post::<Value>(&format!("/v1/sessions/{session}/controls"), &controls) {
                    Ok(_) => self.reply(Response::Mutated(session)),
                    Err(error) => self.reply_failure(error),
                }
            }
            Request::FileDiff { session, scope } => {
                match self.get::<Value>(&format!("/v1/sessions/{session}/diff?scope={scope}")) {
                    Ok(value) => {
                        let patch = value
                            .get("content")
                            .or_else(|| value.get("patch"))
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_owned();
                        self.reply(Response::Diff(session, patch));
                    }
                    Err(error) => self.reply_failure(error),
                }
            }
            Request::ReviewHunks { session } => {
                match self.get::<Value>(&format!("/v1/sessions/{session}/hunks")) {
                    Ok(value) => self.reply(Response::Hunks(session, value)),
                    Err(error) => self.reply_failure(error),
                }
            }
            Request::ReviewHunk {
                session,
                action,
                index,
                patch_digest,
            } => {
                let body = serde_json::json!({
                    "index": index,
                    "patch_digest": patch_digest,
                });
                match self.post::<Value>(&format!("/v1/sessions/{session}/hunks/{action}"), &body) {
                    Ok(_) => self.reply(Response::Mutated(session)),
                    Err(error) => self.reply_failure(error),
                }
            }
            Request::ListModels => self.list_models(),
            Request::StartTerminal {
                working_directory,
                rows,
                cols,
            } => {
                let action = StartTerminalAction {
                    program: None,
                    arguments: Vec::new(),
                    working_directory: std::path::PathBuf::from(working_directory),
                    environment: std::collections::BTreeMap::new(),
                    // The real size, so the first prompt already wraps where the
                    // panel wraps rather than reflowing on the first keystroke.
                    initial_size: TerminalSize { rows, cols },
                    owner: None,
                    background: None,
                };
                let body = serde_json::json!({
                    "workspace_id": WorkspaceId::new(),
                    "action": action,
                });
                match self.post::<Value>("/v1/terminals", &body) {
                    Ok(value) => self.reply(Response::TerminalStarted(value)),
                    Err(error) => self.reply_failure(error),
                }
            }
            Request::PollTerminal { terminal, since } => {
                match self.get::<Value>(&format!("/v1/terminals/{terminal}/output?since={since}")) {
                    // The daemon answers about the terminal in the path and so
                    // does not repeat its id in the body. The GUI holds several
                    // terminals at once, so the reply has to say which one it
                    // belongs to or the output lands in whichever tab is first.
                    Ok(mut value) => {
                        if let Some(object) = value.as_object_mut() {
                            object.insert("terminal_id".into(), Value::String(terminal));
                        }
                        self.reply(Response::TerminalOutput(value));
                    }
                    Err(error) => self.reply_failure(error),
                }
            }
            Request::SendTerminalInput {
                terminal,
                generation,
                bytes,
            } => {
                let body = serde_json::json!({
                    "generation": generation,
                    "bytes": bytes,
                });
                match self.post::<Value>(&format!("/v1/terminals/{terminal}/input"), &body) {
                    Ok(_) => {}
                    Err(error) => self.reply_failure(error),
                }
            }
            Request::ResizeTerminal {
                terminal,
                rows,
                cols,
            } => {
                let body = serde_json::json!({ "rows": rows, "cols": cols });
                match self.post::<Value>(&format!("/v1/terminals/{terminal}/resize"), &body) {
                    Ok(_) => {}
                    Err(error) => self.reply_failure(error),
                }
            }
            Request::StopTerminal { terminal } => {
                match self.delete::<Value>(&format!("/v1/terminals/{terminal}")) {
                    Ok(value) => self.reply(Response::TerminalChanged(value)),
                    Err(error) => self.reply_failure(error),
                }
            }
            Request::SetTerminalOwner {
                terminal,
                agent_role,
            } => {
                let owner = match agent_role {
                    Some(role) => serde_json::json!({"kind": "agent", "data": {"role": role}}),
                    None => serde_json::json!({"kind": "human"}),
                };
                let body = serde_json::json!({ "owner": owner });
                match self.post::<Value>(&format!("/v1/terminals/{terminal}/owner"), &body) {
                    Ok(value) => self.reply(Response::TerminalChanged(value)),
                    Err(error) => self.reply_failure(error),
                }
            }
            Request::ListTerminals => match self.get::<Value>("/v1/terminals") {
                Ok(value) => {
                    let terminals = value
                        .get("terminals")
                        .and_then(Value::as_array)
                        .cloned()
                        .unwrap_or_default();
                    self.reply(Response::Terminals(terminals));
                }
                Err(error) => self.reply_failure(error),
            },
            // ── Settings surfaces (Defect A) ───────────────────────────
            Request::ListProviders => match self.get::<Value>("/v1/providers") {
                Ok(value) => self.reply(Response::Providers(
                    value.as_array().cloned().unwrap_or_default(),
                )),
                Err(error) => self.reply_failure(error),
            },
            Request::GetProvider { name } => {
                match self.get::<Value>(&format!("/v1/providers/{}", urlencode(&name))) {
                    Ok(value) => self.reply(Response::Provider(value)),
                    Err(error) => self.reply_failure(error),
                }
            }
            Request::TestProvider { name } => {
                let body = serde_json::json!({ "provider": name });
                match self.post::<Value>("/v1/providers/test", &body) {
                    Ok(value) => self.reply(Response::ProviderTested(value)),
                    Err(error) => self.reply_failure(error),
                }
            }
            Request::RemoveProvider { name } => {
                match self.delete::<Value>(&format!("/v1/providers/{}", urlencode(&name))) {
                    Ok(_) => {
                        self.reply(Response::SettingsMutated);
                        self.list_models();
                    }
                    Err(error) => self.reply_failure(error),
                }
            }
            Request::DiscoverProviderModels { provider_type } => {
                let body = serde_json::json!({ "provider_type": provider_type });
                match self.post::<Value>("/v1/providers/discover", &body) {
                    Ok(value) => self.reply(Response::DiscoveredModels(value)),
                    Err(error) => self.reply_failure(error),
                }
            }
            Request::LocalModelsStatus => match self.get::<Value>("/v1/local-models") {
                Ok(value) => self.reply(Response::LocalModels(value)),
                Err(error) => self.reply_failure(error),
            },
            Request::LocalModelsRecommendations => {
                match self.get::<Value>("/v1/local-models/recommendations") {
                    Ok(value) => self.reply(Response::LocalModelsRecommendations(value)),
                    Err(error) => self.reply_failure(error),
                }
            }
            Request::LocalModelsQualify { model } => {
                let body = serde_json::json!({ "model": model });
                match self.post_long_timeout::<Value>("/v1/local-models/qualify", &body) {
                    Ok(value) => self.reply(Response::LocalModelsQualified(value)),
                    Err(error) => self.reply_failure(error),
                }
            }
            Request::LocalModelsUnload { model, all } => {
                let body = serde_json::json!({ "model": model, "all": all });
                match self.post::<Value>("/v1/local-models/unload", &body) {
                    Ok(value) => self.reply(Response::LocalModelsUnloaded(value)),
                    Err(error) => self.reply_failure(error),
                }
            }
            Request::LocalModelsPullPropose {
                session_id,
                repository,
                model,
            } => {
                let body = serde_json::json!({
                    "session_id": session_id,
                    "repository": repository,
                    "model": model,
                });
                match self.post::<Value>("/v1/local-models/pull/propose", &body) {
                    Ok(value) => self.reply(Response::LocalModelsPullProposed(value)),
                    Err(error) => self.reply_failure(error),
                }
            }
            Request::LocalModelsPullApprove {
                session_id,
                action_id,
            } => {
                let body = serde_json::json!({ "session_id": session_id });
                match self.post::<Value>(
                    &format!("/v1/local-models/pull/{}/approve", urlencode(&action_id)),
                    &body,
                ) {
                    Ok(value) => self.reply(Response::LocalModelsPullApproved(value)),
                    Err(error) => self.reply_failure(error),
                }
            }
            Request::LocalModelsPullStart {
                session_id,
                action_id,
            } => {
                let body = serde_json::json!({ "session_id": session_id });
                match self.post::<Value>(
                    &format!("/v1/local-models/pull/{}/start", urlencode(&action_id)),
                    &body,
                ) {
                    Ok(value) => self.reply(Response::LocalModelsPullStarted(value)),
                    Err(error) => self.reply_failure(error),
                }
            }
            Request::LocalModelsPullPoll { action_id } => {
                match self.get::<Value>(&format!("/v1/local-models/pull/{}", urlencode(&action_id)))
                {
                    Ok(value) => self.reply(Response::LocalModelsPullProgress(value)),
                    Err(error) => self.reply_failure(error),
                }
            }
            Request::LocalModelsPullCancel {
                session_id,
                action_id,
            } => {
                let body = serde_json::json!({ "session_id": session_id });
                match self.post::<Value>(
                    &format!("/v1/local-models/pull/{}/cancel", urlencode(&action_id)),
                    &body,
                ) {
                    Ok(value) => self.reply(Response::LocalModelsPullCancelled(value)),
                    Err(error) => self.reply_failure(error),
                }
            }
            Request::LocalModelsGetSettings => match self.get::<Value>("/v1/local-models/settings")
            {
                Ok(value) => self.reply(Response::LocalModelsSettings(value)),
                Err(error) => self.reply_failure(error),
            },
            Request::LocalModelsPutSettings { settings } => {
                match self.post::<Value>("/v1/local-models/settings", &settings) {
                    Ok(value) => self.reply(Response::LocalModelsSettings(value)),
                    Err(error) => self.reply_failure(error),
                }
            }
            Request::ListSkills => match self.get::<Value>("/v1/skills") {
                Ok(value) => self.reply(Response::Skills(
                    value.as_array().cloned().unwrap_or_default(),
                )),
                Err(error) => self.reply_failure(error),
            },
            Request::GetSkill { id } => {
                match self.get::<Value>(&format!("/v1/skills/{}", urlencode(&id))) {
                    Ok(value) => self.reply(Response::Skill(value)),
                    Err(error) => self.reply_failure(error),
                }
            }
            Request::RemoveSkill { id } => {
                match self.delete::<Value>(&format!("/v1/skills/{}", urlencode(&id))) {
                    Ok(value) => {
                        self.reply(Response::SkillRemoved(value));
                        self.reply(Response::SettingsMutated);
                    }
                    Err(error) => self.reply_failure(error),
                }
            }
            Request::SkillSearch {
                session_id,
                capability,
                keywords,
                action_id,
            } => {
                let body = serde_json::json!({
                    "session_id": session_id,
                    "capability": capability,
                    "keywords": keywords,
                    "action_id": action_id,
                });
                match self.post::<Value>("/v1/skills/search", &body) {
                    Ok(value) => self.reply(Response::SkillSearch(value)),
                    Err(error) => self.reply_failure(error),
                }
            }
            Request::SkillDownload {
                session_id,
                candidate_id,
                commit,
                action_id,
            } => {
                let body = serde_json::json!({
                    "session_id": session_id,
                    "candidate_id": candidate_id,
                    "commit": commit,
                    "action_id": action_id,
                });
                match self.post::<Value>("/v1/skills/download", &body) {
                    Ok(value) => self.reply(Response::SkillDownloaded(value)),
                    Err(error) => self.reply_failure(error),
                }
            }
            Request::SkillInstallPropose {
                session_id,
                candidate_id,
                version,
                scope,
                source_path,
                content_digest,
                publisher,
                approved_permissions,
                signature,
                publisher_public_key,
            } => {
                let body = serde_json::json!({
                    "session_id": session_id,
                    "candidate_id": candidate_id,
                    "version": version,
                    "scope": scope,
                    "source_path": source_path,
                    "content_digest": content_digest,
                    "publisher": publisher,
                    "approved_permissions": approved_permissions,
                    "signature": signature,
                    "publisher_public_key": publisher_public_key,
                });
                match self.post::<Value>("/v1/skills/install/propose", &body) {
                    Ok(value) => self.reply(Response::SkillInstallProposed(value)),
                    Err(error) => self.reply_failure(error),
                }
            }
            Request::SkillInstallApprove {
                session_id,
                action_id,
            } => {
                let body = serde_json::json!({ "session_id": session_id });
                match self.post::<Value>(
                    &format!("/v1/skills/install/{}/approve", urlencode(&action_id)),
                    &body,
                ) {
                    Ok(value) => self.reply(Response::SkillInstallApproved(value)),
                    Err(error) => self.reply_failure(error),
                }
            }
            Request::SkillInstall {
                session_id,
                action_id,
            } => {
                let body = serde_json::json!({ "session_id": session_id, "action_id": action_id });
                match self.post::<Value>("/v1/skills/install", &body) {
                    Ok(value) => self.reply(Response::SkillInstalled(value)),
                    Err(error) => self.reply_failure(error),
                }
            }
            Request::SkillBlockPublisher { publisher, reason } => {
                let body = serde_json::json!({ "publisher": publisher, "reason": reason });
                match self.post::<Value>("/v1/skills/publishers/block", &body) {
                    Ok(value) => self.reply(Response::SkillPublisherBlocked(value)),
                    Err(error) => self.reply_failure(error),
                }
            }
            Request::McpList => match self.get::<Value>("/v1/mcp/servers") {
                Ok(value) => self.reply(Response::McpServers(value)),
                Err(error) => self.reply_failure(error),
            },
            Request::McpUpsert { server } => match self.post::<Value>("/v1/mcp/servers", &server) {
                Ok(value) => {
                    self.reply(Response::McpServerSaved(value));
                    self.reply(Response::SettingsMutated);
                }
                Err(error) => self.reply_failure(error),
            },
            Request::McpRemove { id } => {
                match self.delete::<Value>(&format!("/v1/mcp/servers/{}", urlencode(&id))) {
                    Ok(value) => {
                        self.reply(Response::McpServerRemoved(value));
                        self.reply(Response::SettingsMutated);
                    }
                    Err(error) => self.reply_failure(error),
                }
            }
            Request::McpProbe { session, server } => {
                let body = serde_json::json!({
                    "server": server,
                    "tool": "__discover__",
                    "arguments": serde_json::json!({}),
                });
                match self
                    .post::<Value>(&format!("/v1/sessions/{}/mcp", urlencode(&session)), &body)
                {
                    Ok(value) => self.reply(Response::McpProbed(value)),
                    Err(error) => self.reply_failure(error),
                }
            }
            // Language-server reads report their own failure rather than the
            // generic one: a machine with no rust-analyzer is a normal, quiet
            // state, and routing it through `reply_failure` would raise a
            // transport notice every time the pointer crossed a token.
            Request::LspServers => match self.get::<Value>("/v1/lsp/servers") {
                Ok(value) => self.reply(Response::LspServers(value)),
                Err(error) => self.reply(Response::LspUnavailable(error)),
            },
            Request::LspOpen { path, root, text } => {
                let body = serde_json::json!({"path": path, "root": root, "text": text});
                match self.post::<Value>("/v1/lsp/open", &body) {
                    Ok(_) => {}
                    Err(error) => self.reply(Response::LspUnavailable(error)),
                }
            }
            Request::LspHover {
                path,
                root,
                line,
                character,
            } => {
                let body = serde_json::json!({
                    "path": path, "root": root,
                    "position": {"line": line, "character": character},
                });
                match self.post::<Value>("/v1/lsp/hover", &body) {
                    Ok(value) => self.reply(Response::LspHover(path, line, character, value)),
                    Err(error) => self.reply(Response::LspUnavailable(error)),
                }
            }
            Request::LspDefinition {
                path,
                root,
                line,
                character,
            } => {
                let body = serde_json::json!({
                    "path": path, "root": root,
                    "position": {"line": line, "character": character},
                });
                match self.post::<Value>("/v1/lsp/definition", &body) {
                    Ok(value) => self.reply(Response::LspDefinition(value)),
                    Err(error) => self.reply(Response::LspUnavailable(error)),
                }
            }
            Request::LspReferences {
                path,
                root,
                line,
                character,
                label,
            } => {
                let body = serde_json::json!({
                    "path": path, "root": root,
                    "position": {"line": line, "character": character},
                });
                match self.post::<Value>("/v1/lsp/references", &body) {
                    Ok(value) => self.reply(Response::LspReferences(label, value)),
                    Err(error) => self.reply(Response::LspUnavailable(error)),
                }
            }
            Request::LspSymbols { path, root } => {
                let body = serde_json::json!({"path": path, "root": root});
                match self.post::<Value>("/v1/lsp/symbols", &body) {
                    Ok(value) => self.reply(Response::LspSymbols(path, value)),
                    Err(error) => self.reply(Response::LspUnavailable(error)),
                }
            }
            Request::LspFormat {
                path,
                root,
                then_save,
            } => {
                let body = serde_json::json!({"path": path, "root": root});
                match self.post::<Value>("/v1/lsp/format", &body) {
                    Ok(value) => self.reply(Response::LspFormat(path, value, then_save)),
                    Err(error) => self.reply(Response::LspUnavailable(error)),
                }
            }
            Request::LspDiagnostics => match self.get::<Value>("/v1/lsp/diagnostics") {
                Ok(value) => self.reply(Response::LspDiagnostics(value)),
                Err(error) => self.reply(Response::LspUnavailable(error)),
            },
            Request::CodexGet => match self.get::<Value>("/v1/codex") {
                Ok(value) => self.reply(Response::Codex(value)),
                Err(error) => self.reply_failure(error),
            },
            Request::CodexPut { config } => match self.post::<Value>("/v1/codex", &config) {
                Ok(value) => {
                    self.reply(Response::CodexSaved(value));
                    self.reply(Response::SettingsMutated);
                }
                Err(error) => self.reply_failure(error),
            },
            Request::CodexDoctor => match self.post::<Value>("/v1/codex/doctor", &Value::Null) {
                Ok(value) => self.reply(Response::CodexDoctor(value)),
                Err(error) => self.reply_failure(error),
            },
        }
    }

    fn get<T: DeserializeOwned>(&self, path: &str) -> Result<T, String> {
        let url = format!("{}{path}", self.connection.base_url);
        let response = self
            .http
            .get(&url)
            .bearer_auth(&self.connection.token)
            .send()
            .map_err(|error| describe(&error))?;
        decode(response, path)
    }

    fn fetch_panel(
        &self,
        kind: PanelKind,
        path: &str,
        panels: &mut BTreeMap<PanelKind, PanelResult>,
    ) -> PanelResult {
        let mut result = match self.get::<Value>(path) {
            Ok(value) => PanelResult::success(value),
            Err(error) => PanelResult::failure(error),
        };
        // These durable-work routes are versioned presentation contracts. A
        // daemon that predates them has a capability/error gap, not an empty
        // spec or task graph; keep the UI on the explicit Error + Retry path.
        if matches!(
            kind,
            PanelKind::Spec | PanelKind::Tasks | PanelKind::Evidence
        ) && result.availability == PanelAvailability::Unavailable
        {
            result.availability = PanelAvailability::Error;
        }
        panels.insert(kind, result.clone());
        result
    }

    fn post<T: DeserializeOwned>(&self, path: &str, body: &Value) -> Result<T, String> {
        let url = format!("{}{path}", self.connection.base_url);
        let response = self
            .http
            .post(&url)
            .bearer_auth(&self.connection.token)
            .json(body)
            .send()
            .map_err(|error| describe(&error))?;
        decode(response, path)
    }

    /// `POST` with a multi-minute timeout. Model qualification can take several
    /// minutes while the provider probes real generation; the shared 30-second
    /// client timeout (used everywhere else) would fail it (FR-A3).
    fn post_long_timeout<T: DeserializeOwned>(
        &self,
        path: &str,
        body: &Value,
    ) -> Result<T, String> {
        let url = format!("{}{path}", self.connection.base_url);
        let client = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(600))
            .no_proxy()
            .pool_max_idle_per_host(0)
            .build()
            .map_err(|error| describe(&error))?;
        let response = client
            .post(&url)
            .bearer_auth(&self.connection.token)
            .json(body)
            .send()
            .map_err(|error| describe(&error))?;
        decode(response, path)
    }

    fn delete<T: DeserializeOwned>(&self, path: &str) -> Result<T, String> {
        let url = format!("{}{path}", self.connection.base_url);
        let response = self
            .http
            .delete(&url)
            .bearer_auth(&self.connection.token)
            .send()
            .map_err(|error| describe(&error))?;
        decode(response, path)
    }
}

fn provider_configuration_body(
    name: String,
    provider_type: String,
    base_url: String,
    model: String,
    credential_name: Option<String>,
    secret: Option<String>,
    replace: bool,
) -> Value {
    serde_json::json!({
        "name": name,
        "provider_type": provider_type,
        "base_url": base_url,
        "model": model,
        "credential_name": credential_name,
        // The IDE sends an inline secret only when the user typed one in the
        // simplified form; the daemon stores it to credentials.toml. It never
        // constructs or forwards a typed credential reference.
        "secret": secret,
        "credential_reference": null,
        "replace": replace,
    })
}

fn model_role_body(role: String, model: String) -> Value {
    serde_json::json!({ "role": role, "model": model })
}

/// Return whether a session-list row is the daemon's explicit quarantine
/// marker for a replay-invalid legacy event log.
///
/// Quarantined logs remain durable in NineLives and are still available to
/// authenticated diagnostics/API callers. They are not usable IDE sessions,
/// though: the daemon rejects opening or appending to them. Suppressing only
/// rows with both the canonical `unavailable` status and a non-empty reason
/// keeps ordinary failed/paused sessions visible and avoids treating an
/// arbitrary missing field as evidence that a record is broken. This is a
/// presentation filter, not a delete: the append-only audit record is untouched.
fn is_quarantined_legacy_session(value: &Value) -> bool {
    let status = value
        .get("status_code")
        .or_else(|| value.get("status"))
        .and_then(Value::as_str)
        .map(str::trim)
        .map(str::to_ascii_lowercase);
    let has_reason = value
        .get("unavailable_reason")
        .and_then(Value::as_str)
        .is_some_and(|reason| !reason.trim().is_empty());
    has_reason && status.as_deref() == Some("unavailable")
}

fn panel_path(kind: PanelKind, session: &str, scope: &str) -> String {
    match kind {
        PanelKind::Summary => format!("/v1/sessions/{session}/summary"),
        PanelKind::Conversation => format!("/v1/sessions/{session}/conversation"),
        PanelKind::Activity => format!("/v1/sessions/{session}/activity"),
        PanelKind::Artifacts => format!("/v1/sessions/{session}/artifacts"),
        PanelKind::Changes => format!("/v1/sessions/{session}/changes?scope={scope}"),
        PanelKind::Validation => format!("/v1/sessions/{session}/validation"),
        PanelKind::Usage => format!("/v1/sessions/{session}/usage"),
        PanelKind::Controls => format!("/v1/sessions/{session}/controls"),
        PanelKind::Github => format!("/v1/sessions/{session}/github"),
        PanelKind::Spec => format!("/v1/sessions/{session}/spec"),
        PanelKind::Tasks => format!("/v1/sessions/{session}/tasks"),
        PanelKind::Evidence => format!("/v1/sessions/{session}/evidence"),
    }
}

fn decode<T: DeserializeOwned>(
    response: reqwest::blocking::Response,
    path: &str,
) -> Result<T, String> {
    let status = response.status();
    if !status.is_success() {
        let detail = response.text().unwrap_or_default();
        let detail = detail.chars().take(300).collect::<String>();
        return Err(format!("{path} returned {status}: {detail}"));
    }
    // Several mutation endpoints answer 202 with an empty body; treat that as a
    // successful unit rather than a parse failure.
    let text = response.text().map_err(|error| describe(&error))?;
    if text.trim().is_empty() {
        return serde_json::from_str::<T>("null")
            .map_err(|_| format!("{path} returned an empty body"));
    }
    serde_json::from_str::<T>(&text).map_err(|error| format!("{path}: {error}"))
}

/// Turn a transport failure into something a person can act on.
///
/// `reqwest::Error`'s own text is the URL it was given — "error sending request
/// for url (http://127.0.0.1:7377/v1/sessions)" — which names a port and
/// explains nothing. The useful detail is in the source chain underneath it,
/// and the useful *message* is what the user should do about it.
fn describe(error: &reqwest::Error) -> String {
    if error.is_connect() {
        return "Cannot reach the PurrCode daemon.".to_owned();
    }
    if error.is_timeout() {
        return "The PurrCode daemon did not respond in time.".to_owned();
    }
    if error.is_decode() {
        return "The PurrCode daemon sent a response this version cannot read.".to_owned();
    }
    message_for_cause(&root_cause(error))
}

/// The sentence for a transport cause.
///
/// Split out from [`describe`] so it can be tested: a `reqwest::Error` cannot
/// be constructed by hand, but the strings hyper produces can.
fn message_for_cause(cause: &str) -> String {
    let lower = cause.to_ascii_lowercase();
    if lower.contains("connection closed") || lower.contains("connection reset") {
        // Hyper only reports this when the connection died before a response,
        // so the request never took effect and repeating it is safe.
        return "The connection to the PurrCode daemon dropped. Try again.".to_owned();
    }
    if lower.contains("broken pipe") || lower.contains("os error 32") {
        return "The PurrCode daemon closed the connection. Try again.".to_owned();
    }
    if cause.trim().is_empty() {
        return "The request to the PurrCode daemon failed.".to_owned();
    }
    format!("The request to the PurrCode daemon failed: {cause}")
}

fn is_transport_failure(message: &str) -> bool {
    let lower = message.to_ascii_lowercase();
    lower.contains("cannot reach the purrcode daemon")
        || lower.contains("connection to the purrcode daemon dropped")
        || lower.contains("purrcode daemon closed the connection")
        || lower.contains("purrcode daemon did not respond in time")
        || lower.starts_with("the request to the purrcode daemon failed")
}

/// The innermost error, which is the one that says what actually went wrong.
fn root_cause(error: &reqwest::Error) -> String {
    let mut source: &dyn std::error::Error = error;
    while let Some(inner) = source.source() {
        source = inner;
    }
    source.to_string()
}

/// Percent-encode a path for a query string.
///
/// Paths contain spaces and, on Windows, backslashes; both break a bare query
/// parameter and would silently select the wrong repository — or none.
fn urlencode(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' => {
                encoded.push(*byte as char);
            }
            other => encoded.push_str(&format!("%{other:02X}")),
        }
    }
    encoded
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};

    use super::{is_transport_failure, message_for_cause};

    fn read_http_request(stream: &mut TcpStream) -> (String, String) {
        let mut headers = Vec::new();
        let mut byte = [0_u8; 1];
        while !headers.ends_with(b"\r\n\r\n") {
            stream
                .read_exact(&mut byte)
                .expect("test client sends complete HTTP headers");
            headers.push(byte[0]);
        }
        let header_text = String::from_utf8(headers).expect("test headers are UTF-8");
        let content_length = header_text
            .lines()
            .find_map(|line| {
                let (name, value) = line.split_once(':')?;
                name.eq_ignore_ascii_case("content-length").then_some(value)
            })
            .and_then(|length| length.trim().parse::<usize>().ok())
            .unwrap_or(0);
        let mut body = vec![0_u8; content_length];
        stream
            .read_exact(&mut body)
            .expect("test client sends complete HTTP body");
        let request_line = header_text
            .lines()
            .next()
            .expect("test request has a request line")
            .to_owned();
        (
            request_line,
            String::from_utf8(body).expect("test body is UTF-8"),
        )
    }

    fn spawn_json_server(
        requests: usize,
    ) -> (String, std::thread::JoinHandle<Vec<(String, String)>>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind test HTTP server");
        let address = format!(
            "http://{}",
            listener.local_addr().expect("test server address")
        );
        let handle = std::thread::spawn(move || {
            let mut seen = Vec::with_capacity(requests);
            for _ in 0..requests {
                let (mut stream, _) = listener.accept().expect("accept test HTTP request");
                let request = read_http_request(&mut stream);
                let payload = if request.0.starts_with("GET /v1/models ") {
                    r#"[{"id":"local/qwen"}]"#
                } else {
                    "{}"
                };
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    payload.len(),
                    payload
                );
                stream
                    .write_all(response.as_bytes())
                    .expect("write test HTTP response");
                stream.flush().expect("flush test HTTP response");
                seen.push(request);
            }
            seen
        });
        (address, handle)
    }

    #[test]
    fn only_transport_failures_clear_the_connected_indicator() {
        assert!(is_transport_failure("Cannot reach the PurrCode daemon."));
        assert!(is_transport_failure(
            "The connection to the PurrCode daemon dropped. Try again."
        ));
        assert!(!is_transport_failure(
            "/v1/sessions returned 400 Bad Request: invalid repository"
        ));
    }

    /// What the user is told when the loopback connection drops.
    ///
    /// The symptom this replaces: a healthy daemon answering curl in four
    /// milliseconds, while the window said "error sending request for url
    /// (http://127.0.0.1:7377/v1/sessions)" — a URL, a port, and no idea what
    /// to do about it.
    #[test]
    fn a_dropped_connection_is_explained_and_not_named_by_url() {
        for cause in [
            "connection closed before message completed",
            "Connection reset by peer (os error 54)",
        ] {
            let message = message_for_cause(cause);
            assert!(
                message.contains("Try again"),
                "a dropped connection is retryable and must say so: {message}"
            );
            assert!(
                !message.contains("http://") && !message.contains("7377"),
                "a URL and a port number are not an explanation: {message}"
            );
        }
    }

    #[test]
    fn an_unrecognised_cause_survives_verbatim() {
        // Never swallow a failure we cannot classify: the raw cause is the
        // only thing left that could identify it.
        let message = message_for_cause("something nobody has seen before");
        assert!(message.contains("something nobody has seen before"));
    }

    #[test]
    fn an_empty_cause_still_produces_a_sentence() {
        assert_eq!(
            message_for_cause("  "),
            "The request to the PurrCode daemon failed."
        );
    }

    use super::*;

    #[test]
    fn a_connection_normalises_a_trailing_slash_so_paths_do_not_double_up() {
        let connection = Connection::new("http://127.0.0.1:7377/", "token");
        assert_eq!(connection.base_url, "http://127.0.0.1:7377");
    }

    #[test]
    fn the_client_never_blocks_when_nothing_has_arrived() {
        let client = DaemonClient::spawn(Connection::new("http://127.0.0.1:1", "t"), || {});
        assert!(client.drain().is_empty());
    }

    #[test]
    fn an_unreachable_daemon_reports_disconnected_rather_than_hanging() {
        // Port 1 is never a PurrCode daemon, so this exercises the connect-error
        // path the UI shows as "PurrCode daemon unavailable".
        let client = DaemonClient::spawn(Connection::new("http://127.0.0.1:1", "t"), || {});
        client.send(Request::Bootstrap);
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        let mut saw_disconnect = false;
        while std::time::Instant::now() < deadline && !saw_disconnect {
            for response in client.drain() {
                if matches!(response, Response::Connected(false)) {
                    saw_disconnect = true;
                }
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(
            saw_disconnect,
            "an unreachable daemon must report disconnected"
        );
    }

    #[test]
    fn panel_results_keep_empty_unavailable_and_error_distinct() {
        assert_eq!(
            PanelResult::success(serde_json::json!([])).availability,
            PanelAvailability::Empty
        );
        assert_eq!(
            PanelResult::success(serde_json::json!({"status": "unavailable"})).availability,
            PanelAvailability::Unavailable
        );
        let failed = PanelResult::failure("/v1/sessions/s/validation returned 503: busy".into());
        assert_eq!(failed.availability, PanelAvailability::Error);
        assert!(
            failed
                .error
                .as_deref()
                .is_some_and(|detail| detail.contains("503"))
        );
    }

    #[test]
    fn a_loading_snapshot_marks_every_panel_loading() {
        let snapshot = SessionSnapshot::loading();
        assert_eq!(snapshot.panels.len(), PanelKind::ALL.len());
        assert!(
            PanelKind::ALL
                .iter()
                .all(|kind| { snapshot.panel(*kind).availability == PanelAvailability::Loading })
        );
    }

    #[test]
    fn panel_paths_keep_scope_only_on_changes() {
        assert_eq!(
            panel_path(PanelKind::Changes, "s", "agent"),
            "/v1/sessions/s/changes?scope=agent"
        );
        assert_eq!(
            panel_path(PanelKind::Summary, "s", "agent"),
            "/v1/sessions/s/summary"
        );
        assert_eq!(
            panel_path(PanelKind::Spec, "s", "agent"),
            "/v1/sessions/s/spec"
        );
        assert_eq!(
            panel_path(PanelKind::Tasks, "s", "agent"),
            "/v1/sessions/s/tasks"
        );
        assert_eq!(
            panel_path(PanelKind::Evidence, "s", "agent"),
            "/v1/sessions/s/evidence"
        );
    }

    #[test]
    fn shared_typed_panel_state_is_not_reinterpreted_as_ready() {
        let empty = PanelResult::success(serde_json::json!({
            "state": "empty",
            "message": "checked and found no evidence",
            "observed_at": "2026-08-02T00:00:00Z"
        }));
        assert_eq!(empty.availability, PanelAvailability::Empty);
        assert_eq!(empty.fetched_at.as_deref(), Some("2026-08-02T00:00:00Z"));

        let error = PanelResult::success(serde_json::json!({
            "state": "error",
            "message": "evidence query timed out",
            "retryable": true
        }));
        assert_eq!(error.availability, PanelAvailability::Error);
        assert_eq!(error.error.as_deref(), Some("evidence query timed out"));
    }

    #[test]
    fn snapshot_pressure_stays_within_the_bounded_query_queue() {
        let (responses, inbound) = mpsc::channel();
        let worker = Worker {
            http: reqwest::blocking::Client::builder()
                .build()
                .expect("test HTTP client"),
            connection: Connection::new("http://127.0.0.1:1", "test"),
            responses,
            repaint: Arc::new(|| {}),
        };
        let (jobs, queue) = mpsc::sync_channel(QUERY_QUEUE_CAPACITY);
        for index in 0..100 {
            dispatch_query(
                Request::LoadSession {
                    session: format!("session-{index}"),
                    scope: "agent",
                },
                &worker,
                &jobs,
            )
            .expect("dispatching a snapshot does not perform I/O");
        }
        // Every request emits its Loading snapshot immediately. Once the
        // finite queue fills, remaining panels receive explicit Error results
        // rather than spawning more threads or silently disappearing.
        let loading_snapshots = inbound
            .try_iter()
            .filter(|response| matches!(response, Response::SessionLoading(_, _, _)))
            .count();
        assert_eq!(loading_snapshots, 100);
        assert_eq!(queue.try_iter().count(), QUERY_QUEUE_CAPACITY);
        assert_eq!(QUERY_WORKERS, 4, "query worker count must stay bounded");
        assert_eq!(CONTROL_WORKERS, 1);
        assert_eq!(URGENT_CONTROL_WORKERS, 1);
    }

    #[test]
    fn session_load_completes_after_all_panels_without_waiting_for_ui_timeout() {
        let started_at = std::time::Instant::now();
        let (responses, inbound) = mpsc::channel();
        let worker = Worker {
            http: reqwest::blocking::Client::builder()
                .build()
                .expect("test HTTP client"),
            connection: Connection::new("http://127.0.0.1:1", "test"),
            responses,
            repaint: Arc::new(|| {}),
        };
        let (jobs, queue) = mpsc::sync_channel(PanelKind::ALL.len());
        dispatch_query(
            Request::LoadSession {
                session: "historical-session".into(),
                scope: "agent",
            },
            &worker,
            &jobs,
        )
        .expect("dispatching a snapshot does not perform I/O");

        let (session, generation, snapshot) = match inbound
            .recv_timeout(Duration::from_secs(1))
            .expect("loading snapshot arrives immediately")
        {
            Response::SessionLoading(session, generation, snapshot) => {
                (session, generation, snapshot)
            }
            other => panic!("expected SessionLoading, got {other:?}"),
        };
        assert_eq!(session, "historical-session");
        assert_eq!(snapshot.panels.len(), PanelKind::ALL.len());

        let queued: Vec<_> = queue.try_iter().collect();
        assert_eq!(queued.len(), PanelKind::ALL.len());
        for (index, job) in queued.into_iter().enumerate() {
            let QueryJob::Panel {
                session: panel_session,
                generation: panel_generation,
                panel,
                load,
                ..
            } = job
            else {
                panic!("LoadSession must enqueue only panel jobs");
            };
            assert_eq!(panel_session, session);
            assert_eq!(panel_generation, generation);
            worker.reply(Response::SessionPanel(
                panel_session,
                panel_generation,
                panel,
                PanelResult::failure("test panel response".into()),
            ));
            worker.finish_session_load(&load);
            if index + 1 < PanelKind::ALL.len() {
                assert!(
                    inbound
                        .try_iter()
                        .all(|response| !matches!(response, Response::SessionLoaded(_, _))),
                    "completion must wait for every panel"
                );
            }
        }

        let completions: Vec<_> = inbound
            .try_iter()
            .filter_map(|response| match response {
                Response::SessionLoaded(session, generation) => Some((session, generation)),
                _ => None,
            })
            .collect();
        assert_eq!(completions, vec![(session, generation)]);
        assert!(
            started_at.elapsed() < Duration::from_secs(1),
            "completion is an explicit event, not the UI's eight-second fallback"
        );
    }

    #[test]
    fn overlapping_loads_keep_panel_and_completion_generations_separate() {
        let (responses, inbound) = mpsc::channel();
        let worker = Worker {
            http: reqwest::blocking::Client::builder()
                .build()
                .expect("test HTTP client"),
            connection: Connection::new("http://127.0.0.1:1", "test"),
            responses,
            repaint: Arc::new(|| {}),
        };
        let (jobs, queue) = mpsc::sync_channel(PanelKind::ALL.len() * 2);
        for _ in 0..2 {
            dispatch_query(
                Request::LoadSession {
                    session: "same-session".into(),
                    scope: "agent",
                },
                &worker,
                &jobs,
            )
            .expect("dispatching an overlapping snapshot does not perform I/O");
        }
        let starts: Vec<_> = inbound
            .try_iter()
            .filter_map(|response| match response {
                Response::SessionLoading(session, generation, _) => Some((session, generation)),
                _ => None,
            })
            .collect();
        assert_eq!(starts.len(), 2);
        assert_ne!(starts[0].1, starts[1].1);

        let queued: Vec<_> = queue.try_iter().collect();
        assert_eq!(queued.len(), PanelKind::ALL.len() * 2);
        // Complete in reverse order to model an older request racing a newer
        // one. Every completion remains tagged with its own load generation.
        for job in queued.into_iter().rev() {
            let QueryJob::Panel {
                session,
                generation,
                panel,
                load,
                ..
            } = job
            else {
                panic!("LoadSession must enqueue panel jobs");
            };
            worker.reply(Response::SessionPanel(
                session,
                generation,
                panel,
                PanelResult::failure("test panel response".into()),
            ));
            worker.finish_session_load(&load);
        }

        let completions: Vec<_> = inbound
            .try_iter()
            .filter_map(|response| match response {
                Response::SessionLoaded(session, generation) => Some((session, generation)),
                _ => None,
            })
            .collect();
        assert_eq!(completions.len(), 2);
        assert!(
            completions
                .iter()
                .all(|(session, _)| session == "same-session")
        );
        assert!(
            completions
                .iter()
                .any(|(_, generation)| *generation == starts[0].1)
        );
        assert!(
            completions
                .iter()
                .any(|(_, generation)| *generation == starts[1].1)
        );
    }

    #[test]
    fn provider_configuration_and_role_requests_use_the_control_lane() {
        assert!(
            Request::ConfigureProvider {
                name: "local".into(),
                provider_type: "ollama".into(),
                base_url: "http://127.0.0.1:11434".into(),
                model: "qwen".into(),
                credential_name: None,
                secret: None,
                replace: false,
            }
            .is_control()
        );
        assert!(
            Request::AssignModelRole {
                role: "coding_worker".into(),
                model: "local/qwen".into(),
            }
            .is_control()
        );
        assert!(
            !Request::AssignModelRole {
                role: "coding_worker".into(),
                model: "local/qwen".into(),
            }
            .is_urgent_control()
        );
    }

    #[test]
    fn provider_configuration_body_has_explicit_safe_credential_fields() {
        let body = provider_configuration_body(
            "local".into(),
            "ollama".into(),
            "http://127.0.0.1:11434".into(),
            "qwen".into(),
            Some("purrcode-ollama".into()),
            Some("sk-inline".into()),
            true,
        );
        assert_eq!(body["name"], "local");
        assert_eq!(body["credential_name"], "purrcode-ollama");
        assert_eq!(body["secret"], "sk-inline");
        assert!(body["credential_reference"].is_null());
        assert_eq!(body["replace"], true);

        let role = model_role_body("coding_worker".into(), "local/qwen".into());
        assert_eq!(
            role,
            serde_json::json!({
                "role": "coding_worker",
                "model": "local/qwen"
            })
        );
    }

    #[test]
    fn session_lists_hide_only_explicitly_quarantined_legacy_rows() {
        let rows = [
            serde_json::json!({"id": "healthy", "status_code": "completed"}),
            serde_json::json!({
                "id": "broken",
                "status_code": "unavailable",
                "unavailable_reason": "event log cannot be replayed"
            }),
            // A status without the daemon's explicit quarantine evidence is
            // retained, so an older/partial response cannot hide user work.
            serde_json::json!({"id": "ambiguous", "status_code": "unavailable"}),
            serde_json::json!({
                "id": "failed",
                "status_code": "failed",
                "unavailable_reason": "provider stopped"
            }),
        ];

        let visible_ids: Vec<_> = rows
            .iter()
            .filter(|row| !is_quarantined_legacy_session(row))
            .filter_map(|row| row.get("id").and_then(Value::as_str))
            .collect();

        assert_eq!(visible_ids, ["healthy", "ambiguous", "failed"]);
        assert!(is_quarantined_legacy_session(&rows[1]));
    }

    #[test]
    fn provider_mutations_hit_their_routes_and_refresh_models() {
        let (base_url, server) = spawn_json_server(4);
        let (responses, inbound) = mpsc::channel();
        let worker = Worker {
            http: reqwest::blocking::Client::builder()
                .no_proxy()
                .build()
                .expect("test HTTP client"),
            connection: Connection::new(base_url, "test-token"),
            responses,
            repaint: Arc::new(|| {}),
        };

        worker.handle(Request::ConfigureProvider {
            name: "local".into(),
            provider_type: "ollama".into(),
            base_url: "http://127.0.0.1:11434".into(),
            model: "qwen".into(),
            credential_name: None,
            secret: None,
            replace: false,
        });
        worker.handle(Request::AssignModelRole {
            role: "coding_worker".into(),
            model: "local/qwen".into(),
        });

        let requests = server.join().expect("test HTTP server completes");
        assert_eq!(requests.len(), 4);
        assert_eq!(requests[0].0, "POST /v1/providers HTTP/1.1");
        let provider_body: Value = serde_json::from_str(&requests[0].1).expect("provider JSON");
        assert_eq!(provider_body["replace"], false);
        assert!(provider_body["credential_reference"].is_null());
        assert_eq!(requests[1].0, "GET /v1/models HTTP/1.1");
        assert_eq!(requests[2].0, "POST /v1/models/roles HTTP/1.1");
        let role_body: Value = serde_json::from_str(&requests[2].1).expect("role JSON");
        assert_eq!(
            role_body,
            serde_json::json!({
                "role": "coding_worker",
                "model": "local/qwen"
            })
        );
        assert_eq!(requests[3].0, "GET /v1/models HTTP/1.1");

        let models: Vec<_> = inbound
            .try_iter()
            .filter_map(|response| match response {
                Response::Models(models) => Some(models),
                // ConfigureProvider now also signals the settings surface to
                // re-fetch its provider list.
                Response::SettingsMutated => None,
                other => panic!("unexpected provider response: {other:?}"),
            })
            .collect();
        assert_eq!(models.len(), 2);
        assert_eq!(models[0][0]["id"], "local/qwen");
        assert_eq!(models[1][0]["id"], "local/qwen");
    }

    #[test]
    fn safety_controls_bypass_the_query_dispatcher() {
        assert!(
            Request::SessionAction {
                session: "s".into(),
                action: "cancel",
                body: Value::Null,
            }
            .is_urgent_control()
        );
        assert!(
            Request::SessionAction {
                session: "s".into(),
                action: "approve",
                body: Value::Null,
            }
            .is_urgent_control()
        );
        // Resume is a durable mutation like recovery and follow-up. It stays
        // on the ordinary control worker; only immediate safety and approval
        // decisions bypass that queue.
        assert!(
            Request::SessionAction {
                session: "s".into(),
                action: "resume",
                body: Value::Null,
            }
            .is_control()
        );
        assert!(
            !Request::SessionAction {
                session: "s".into(),
                action: "resume",
                body: Value::Null,
            }
            .is_urgent_control()
        );
        assert!(
            Request::SendTerminalInput {
                terminal: "t".into(),
                generation: 1,
                bytes: vec![3],
            }
            .is_urgent_control()
        );
        assert!(
            Request::SetTerminalOwner {
                terminal: "t".into(),
                agent_role: None,
            }
            .is_urgent_control()
        );
        assert!(
            !Request::LoadSession {
                session: "s".into(),
                scope: "agent",
            }
            .is_control()
        );
    }
}
