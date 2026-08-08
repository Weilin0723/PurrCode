//! The application: one window, one folder, one conversation.
//!
//! The shape of this module is the product argument of PRD §24 and §15. The
//! window is a hierarchy, not a dashboard: the user's intent and the work
//! answering it own the centre, the code and the changes sit beside it, and
//! everything that is only *evidence* — tests, problems, terminals, raw
//! activity — lives in a dock the user opens when they want it.
//!
//! Nothing here decides how the agent works. The composer sends an intent and
//! the daemon resolves the workflow (PRD §5, §7): this client never asks the
//! user to pick Ask, Build or Plan before they are allowed to type, and it
//! never renders a control whose only purpose is to tell the runtime something
//! the runtime can work out for itself (PRD §10).

mod bar;
pub(crate) mod checkpoints;
mod code;
mod composer;
mod dock;
mod editor;
mod errors;
pub(crate) mod files;
pub(crate) mod language;
mod navigation;
pub(crate) mod primitives;
mod settings;
mod terminals;
mod workbench;

pub use errors::{Notice, NoticeAction, NoticeKind};

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use egui::{Align, Context, Layout, RichText, Sense, Ui};
use serde_json::Value;

use crate::daemon::{Connection, DaemonClient, PanelKind, Request, Response};
use crate::model::{self, ChangeScope, Session, SessionRow};
use crate::terminal::GuiTerminal;
use crate::theme::{self, Appearance, Tokens};
use crate::welcome::{RecentWorkspaces, WelcomeChoice};

/// How the window was opened.
#[derive(Clone, Debug)]
pub struct Config {
    /// The folder to work in. `None` opens the start screen instead of
    /// silently adopting whatever directory the shell happened to be in.
    pub repository: Option<PathBuf>,
    pub base_url: String,
    pub token: String,
    pub initial_session: Option<String>,
}

/// What the window is currently for.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Stage {
    /// No folder open: the navigation column asks for one.
    Welcome,
    /// A folder is open and the product is usable.
    Workspace,
}

/// The vertical Activity Bar rail — the top-level navigator of the IDE shell.
/// Each item switches what the primary sidebar shows (PRD §9-13).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ActivityBar {
    /// Agent sessions — the signature surface.
    Agent,
    /// Project file tree.
    Explorer,
    /// Repository-wide text search.
    Search,
    /// Git changes.
    SourceControl,
}

impl ActivityBar {
    pub(crate) const ALL: &'static [Self] = &[
        Self::Agent,
        Self::Explorer,
        Self::Search,
        Self::SourceControl,
    ];

    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Agent => "Agent",
            Self::Explorer => "Explorer",
            Self::Search => "Search",
            Self::SourceControl => "Source Control",
        }
    }

    pub(crate) const fn glyph(self) -> crate::icons::Glyph {
        match self {
            // Not the mascot: the mascot is the brand mark at the top of the
            // same rail, and two of it in one column reads as a mistake.
            Self::Agent => crate::icons::Glyph::Conversation,
            Self::Explorer => crate::icons::Glyph::Files,
            Self::Search => crate::icons::Glyph::Search,
            Self::SourceControl => crate::icons::Glyph::Branch,
        }
    }

    pub(crate) const fn shortcut(self) -> &'static str {
        match self {
            Self::Agent => "1",
            Self::Explorer => "2",
            Self::Search => "3",
            Self::SourceControl => "4",
        }
    }
}

/// Where the single AgentSession renders. One session model, three positions:
/// an editor tab, the right auxiliary panel, or the Agent-Focus main view
/// (PRD §28-29, §34).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AgentLocation {
    EditorTab,
    Aux,
    Focus,
}

/// What the right auxiliary panel shows. `Agent` renders the one session with
/// contextual tabs; `Source` reuses the change-review / file surface.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AuxView {
    Agent,
    Source,
    /// Every use of one symbol, from `textDocument/references`.
    References,
    /// The active document's symbols, from `textDocument/documentSymbol`.
    Outline,
    /// Restorable checkpoints for the selected session.
    Checkpoints,
}

/// Keep the Agent transcript mounted in exactly one location per frame.
///
/// The toolbar can be driven by a stale click while a tab transition is being
/// committed. In that short window an `AuxView::Agent` value may coexist with
/// an editor/focus Agent location; rendering both would duplicate the
/// universal composer widget ID. The source surface is a safe, useful
/// fallback until the explicit split action moves the Agent to the aux dock.
fn effective_aux_view(view: AuxView, location: AgentLocation) -> AuxView {
    if view == AuxView::Agent && !matches!(location, AgentLocation::Aux) {
        AuxView::Source
    } else {
        view
    }
}

/// The evidence dock. Closed by default: PRD §12 classifies all of this as
/// secondary or below, and §15 forbids giving it the same weight as the work.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DockTab {
    Tests,
    Problems,
    Terminal,
    Activity,
}

impl DockTab {
    pub(crate) const ALL: &'static [Self] =
        &[Self::Tests, Self::Problems, Self::Terminal, Self::Activity];

    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Tests => "Tests",
            Self::Problems => "Problems",
            Self::Terminal => "Terminal",
            Self::Activity => "Activity",
        }
    }

    pub(crate) const fn shortcut(self) -> &'static str {
        match self {
            Self::Tests => "4",
            Self::Problems => "5",
            Self::Terminal => "6",
            Self::Activity => "7",
        }
    }
}

/// What the right-hand column is showing.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CodePanel {
    /// Change Review: the first-class completion surface of PRD §21.
    Changes,
    /// A file the user opened from the tree or from a diff.
    Source,
}

/// The folder's own Git state, which exists before any session does.
#[derive(Clone, Debug, Default)]
pub struct WorkspaceView {
    pub branch: Option<String>,
    pub is_git_repository: bool,
    /// `false` when the check could not run — not the same as "clean".
    pub git_known: bool,
    pub clean: bool,
    pub changed_file_count: u64,
    pub recent_commits: Vec<(String, String)>,
    pub github_remote_configured: bool,
}

impl WorkspaceView {
    fn parse(value: &Value) -> Self {
        let git = value.get("git");
        let status = git
            .and_then(|git| git.get("status"))
            .and_then(Value::as_str)
            .unwrap_or("");
        Self {
            branch: value
                .get("branch")
                .and_then(Value::as_str)
                .map(str::to_owned)
                .filter(|branch| !branch.is_empty()),
            is_git_repository: value
                .get("is_git_repository")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            git_known: status == "ready",
            clean: git
                .and_then(|git| git.get("clean"))
                .and_then(Value::as_bool)
                .unwrap_or(true),
            changed_file_count: git
                .and_then(|git| git.get("changed_file_count"))
                .and_then(Value::as_u64)
                .unwrap_or(0),
            recent_commits: git
                .and_then(|git| git.get("recent_commits"))
                .and_then(Value::as_array)
                .map(|commits| {
                    commits
                        .iter()
                        .filter_map(|commit| {
                            Some((
                                commit.get("short_hash")?.as_str()?.to_owned(),
                                commit.get("subject")?.as_str()?.to_owned(),
                            ))
                        })
                        .collect()
                })
                .unwrap_or_default(),
            github_remote_configured: value
                .get("github")
                .and_then(|github| github.get("remote_configured"))
                .and_then(Value::as_bool)
                .unwrap_or(false),
        }
    }
}

/// One file open in the source column.
pub struct OpenFile {
    pub path: PathBuf,
    /// Repository-relative, for the tab label.
    pub label: String,
    pub body: Result<String, String>,
    pub scroll_to_line: Option<usize>,
    /// Whether the editor buffer diverged from disk (tab "●").
    pub modified: bool,
    /// The file's size and modification time when this buffer was last read
    /// or written, so a change made outside the editor can be detected.
    pub disk_stamp: Option<(u64, std::time::SystemTime)>,
    /// Set when the file on disk moved on under an open buffer — an agent
    /// wrote it, a rebase landed, another editor saved. The editor never
    /// silently discards either version; it tells the user and lets them pick.
    pub external_change: bool,
}

/// One entry in the command palette.
#[derive(Clone, Debug)]
pub(crate) struct PaletteCommand {
    pub label: String,
    pub description: String,
    pub action: PaletteAction,
}

/// What choosing a palette entry does.
#[derive(Clone, Debug)]
pub(crate) enum PaletteAction {
    QuickOpen,
    Outline,
    Format,
    Search,
    Terminal,
    Problems,
    Settings,
    /// Place a daemon session command in the composer, ready to send.
    Compose(String),
}

/// The size and modification time of a file, for change detection.
///
/// `None` when the file cannot be stated (deleted, or on a filesystem that
/// does not report mtime). A missing stamp disables the comparison rather than
/// producing a spurious "changed on disk" every frame.
pub(crate) fn disk_stamp(path: &std::path::Path) -> Option<(u64, std::time::SystemTime)> {
    let metadata = std::fs::metadata(path).ok()?;
    Some((metadata.len(), metadata.modified().ok()?))
}

/// A model the user could choose. `Auto` is not in this list: it is the
/// absence of an override (PRD §27).
#[derive(Clone, Debug)]
pub struct ModelChoice {
    pub id: String,
    pub provider: String,
    pub is_default: bool,
    pub local: bool,
    pub roles: Vec<String>,
}

/// The startup state model of PRD §17, as the window can observe it.
///
/// The user sees "Starting PurrCode…", never a loopback URL. The failure path
/// is a sentence and two buttons, never a transport error (PRD §18).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Startup {
    /// Talking to the local service for the first time.
    Connecting,
    /// The service answered; the workspace is being restored.
    Restoring,
    /// Usable.
    Ready,
    /// The service never answered. Recovery is offered, not diagnosed at the
    /// user.
    Unavailable,
}

pub struct PurrCodeIde {
    // ── Connection ─────────────────────────────────────────────────────
    pub(crate) client: DaemonClient,
    pub(crate) connected: bool,
    pub(crate) startup: Startup,
    pub(crate) startup_began: Instant,
    /// The last raw transport failure. Never rendered as a headline — it is
    /// the "Technical details" of a notice (PRD §18).
    pub(crate) last_transport_error: Option<String>,

    // ── Appearance ─────────────────────────────────────────────────────
    pub(crate) appearance: Appearance,
    pub(crate) pending_appearance: Appearance,
    pub(crate) tokens: Tokens,

    // ── Workspace ──────────────────────────────────────────────────────
    pub(crate) stage: Stage,
    pub(crate) repository: PathBuf,
    pub(crate) recents: RecentWorkspaces,
    pub(crate) welcome_error: Option<String>,
    pub(crate) workspace_state: WorkspaceView,

    // ── Work ───────────────────────────────────────────────────────────
    pub(crate) sessions: Vec<SessionRow>,
    pub(crate) selected: Option<String>,
    pub(crate) session: Session,
    pub(crate) session_loading: bool,
    pub(crate) submitting: bool,

    // ── Composer ───────────────────────────────────────────────────────
    pub(crate) composer: String,
    /// The exact draft currently in flight. A failed daemon mutation restores
    /// this text instead of making the user's request disappear.
    pub(crate) pending_submission: Option<String>,
    /// Text handed back after a failed submission, so nothing a user typed is
    /// ever lost to a transport error.
    pub(crate) focus_composer: bool,
    /// True while an input-method editor (e.g. pinyin) is composing a string.
    /// Enter confirms a candidate and Esc cancels the composition, so the
    /// composer's Enter-to-send / Esc-to-clear must not fire during it.
    pub(crate) ime_composing: bool,
    /// The caret's byte offset in the composer, for `/ @ #` completion.
    pub(crate) composer_caret: usize,
    /// The token the completion popup is offering suggestions for.
    pub(crate) active_completion: Option<composer::ActiveToken>,
    /// Which suggestion the arrow keys have landed on.
    pub(crate) completion_index: usize,
    /// Commands the daemon publishes, as `(name, description, group)`.
    pub(crate) commands: Vec<(String, String, String)>,
    /// Project-wide symbols for `#` completion, as `(name, kind, container)`.
    pub(crate) workspace_symbols: Vec<(String, String, Option<String>)>,
    /// The query `workspace_symbols` answers, so a stale reply is discarded.
    pub(crate) workspace_symbols_for: Option<String>,
    /// The draft's references, resolved by the daemon.
    pub(crate) resolved_references: Vec<model::ResolvedReference>,
    /// The draft `resolved_references` describes, so a reply for text the
    /// user has since edited cannot be shown as current.
    pub(crate) references_requested_for: Option<String>,

    // ── IDE Shell ────────────────────────────────────────────────────────
    /// The selected Activity Bar item — drives the primary sidebar.
    pub(crate) activity: ActivityBar,
    pub(crate) sidebar_visible: bool,
    /// Where the single AgentSession renders (editor tab / aux panel / focus).
    pub(crate) agent_location: AgentLocation,
    /// Shows a dirty dot on the Agent tab when its session has new activity
    /// while another tab is active.
    pub(crate) agent_tab_dirty: bool,
    /// Right auxiliary panel; `None` = closed.
    pub(crate) aux_panel: Option<AuxView>,
    /// Bottom panel; `None` = closed (reuses DockTab for its kind).
    pub(crate) bottom_panel: Option<DockTab>,
    /// Paths of files edited in the editor (tab "●").
    pub(crate) dirty: BTreeSet<PathBuf>,
    /// A file close waiting on a "discard changes?" modal. `None` when no
    /// confirmation is pending; the index is the row in `open_files`.
    pub(crate) pending_close: Option<usize>,
    /// Command palette query (Cmd+Shift+P); `None` = closed.
    pub(crate) command_palette: Option<String>,
    /// Quick-open query (Cmd+P); `None` = closed.
    pub(crate) quick_open: Option<String>,
    pub(crate) title_visible: bool,

    // ── Navigation ─────────────────────────────────────────────────────
    pub(crate) expanded: BTreeSet<PathBuf>,
    pub(crate) search_query: String,
    pub(crate) search_results: Vec<(PathBuf, usize, String)>,
    pub(crate) search_ran: bool,
    /// The query the last search actually ran, so an edited query re-runs.
    pub(crate) search_ran_for: Option<String>,

    // ── Code and changes ───────────────────────────────────────────────
    pub(crate) code_panel: CodePanel,
    pub(crate) open_files: Vec<OpenFile>,
    pub(crate) active_file: usize,
    pub(crate) inline_diff_open: bool,
    pub(crate) diff_scope: ChangeScope,
    pub(crate) diff: Option<String>,
    pub(crate) diff_loading: bool,
    /// Set on session/workspace switch so the next frame clears the
    /// `SELECTED_DIFF_ID` temp-data key (which needs the egui Context).
    pub(crate) pending_clear_selected_diff: bool,
    /// Per-file change counts for the open folder (the user's own checkout),
    /// from `GET /v1/workspace/changes`. Distinct from `session.changes`, which
    /// describes the selected session's worktree.
    pub(crate) workspace_changes: model::Changes,
    /// `true` while a workspace-changes request is in flight, so a retry shows
    /// honest loading state rather than re-issuing an identical poll.
    pub(crate) workspace_changes_loading: bool,
    /// `true` once the first workspace-changes response has landed, so the
    /// Changes panel can tell "still loading" (show a spinner) from "genuinely
    /// has nothing to report" (show the file tree).
    pub(crate) workspace_changes_checked: bool,

    // ── Session workspace ──────────────────────────────────────────────
    /// The sidebar's search box. Non-empty means the list is replaced by hits.
    pub(crate) session_query: String,
    pub(crate) session_hits: Vec<model::SessionHit>,
    /// The query `session_hits` answers, so a reply for an abandoned query is
    /// never shown against a newer one.
    pub(crate) session_hits_for: Option<String>,
    /// A rename in progress, as `(session id, draft title)`.
    pub(crate) renaming_session: Option<(String, String)>,
    /// A delete awaiting confirmation, as `(session id, title)`.
    pub(crate) deleting_session: Option<(String, String)>,

    // ── Checkpoints ────────────────────────────────────────────────────
    /// Restorable points for the selected session, newest first.
    pub(crate) checkpoints: Vec<model::Checkpoint>,
    /// The session `checkpoints` belongs to, so a switch refetches.
    pub(crate) checkpoints_for: Option<String>,
    /// A restore awaiting confirmation.
    pub(crate) pending_restore: Option<checkpoints::PendingRestore>,

    // ── Language intelligence ──────────────────────────────────────────
    /// What language servers have told this window (v1.2 Pillar 1).
    pub(crate) language: crate::model::LanguageIntelligence,
    /// Where the pointer is resting in the editor, for delayed hover.
    pub(crate) hover_probe: Option<language::HoverProbe>,
    /// The caret's document position, kept per frame so keyboard actions
    /// (definition, references, rename) know what the user means by "here".
    pub(crate) caret: Option<crate::model::DocumentPosition>,
    /// The identifier under the caret, so a references panel can name what it
    /// is listing without asking the server twice.
    pub(crate) caret_word: String,
    /// Run the formatter on ⌘S before writing to disk.
    pub(crate) format_on_save: bool,

    // ── Explorer file operations ───────────────────────────────────────
    /// The create/rename/delete waiting on the user's confirmation.
    pub(crate) file_operation: Option<files::FileOperation>,
    pub(crate) file_operation_name: String,
    /// Why the last attempt was refused, shown beside the name field.
    pub(crate) file_operation_error: Option<String>,

    // ── Dock ───────────────────────────────────────────────────────────
    pub(crate) dock: Option<DockTab>,

    // ── Terminals ──────────────────────────────────────────────────────
    pub(crate) terminals: Vec<GuiTerminal>,
    pub(crate) active_terminal: usize,
    /// Set when a terminal has just opened, so the grid takes the keyboard
    /// without the user having to click the black rectangle first.
    pub(crate) focus_terminal: bool,

    // ── Advanced surfaces ──────────────────────────────────────────────
    pub(crate) settings_open: bool,
    pub(crate) settings_page: settings::SettingsPage,
    pub(crate) settings_search: String,
    pub(crate) provider_base_url: String,
    pub(crate) provider_model: String,
    pub(crate) provider_api_key: String,
    pub(crate) discover_type: String,
    pub(crate) diagnostics_open: bool,
    pub(crate) models: Vec<ModelChoice>,
    pub(crate) bootstrap: Value,
    /// The settings surface's own fetched state (Defect A / FR-A1).
    pub(crate) settings_state: crate::app::settings::SettingsState,
    // ── Settings form inputs (Defect A) ───────────────────────────────
    /// Per-model chosen role (FR-A2). The role list comes from the daemon's
    /// canonical set, not just `coding_worker`.
    pub(crate) provider_role: std::collections::BTreeMap<String, String>,
    pub(crate) pull_model_input: String,
    pub(crate) local_policy: Option<String>,
    pub(crate) local_idle_timeout: Option<u64>,
    pub(crate) skill_capability_input: String,
    pub(crate) skill_publisher_input: String,
    pub(crate) codex_binary: String,
    pub(crate) codex_timeout: u64,
    pub(crate) codex_inherit_auth: bool,
    pub(crate) codex_require_diff: bool,
    pub(crate) codex_allow_tree_write: bool,
    pub(crate) mcp_id: String,
    pub(crate) mcp_program: String,
    pub(crate) mcp_arguments: String,
    pub(crate) mcp_working_directory: String,
    pub(crate) mcp_network: bool,
    pub(crate) mcp_environment: String,

    // ── Notices ────────────────────────────────────────────────────────
    pub(crate) notices: Vec<Notice>,

    // ── Timing ─────────────────────────────────────────────────────────
    last_session_poll: Instant,
    last_list_poll: Instant,
    last_terminal_poll: Instant,
    /// Workspace change counts are polled at the workspace cadence (same 4 s as
    /// the session list), not the 700 ms session cadence: `git diff --numstat`
    /// over a dirty tree is not free.
    last_workspace_changes_poll: Instant,
    /// Diagnostics are pushed by language servers, so the window polls for
    /// what has been published rather than requesting analysis.
    last_diagnostic_poll: Instant,
    /// When open buffers were last compared against the files on disk.
    last_disk_check: Instant,
    /// When the current `session_loading` began, so a stuck load falls back
    /// instead of spinning forever.
    session_loading_began: Option<Instant>,
    /// Correlates bounded panel responses so a late history load cannot clear
    /// the spinner or overwrite state for a newer selection.
    current_session_load_generation: Option<u64>,
    /// The newest load generation accepted by this window. Unlike
    /// `current_session_load_generation`, this survives a selection change so
    /// a late response from an earlier visit to the same session cannot become
    /// the new snapshot after the user switches away and back.
    last_session_load_generation: Option<u64>,
    /// A load remains in flight until its generation emits `SessionLoaded`.
    /// Polling must not start another fan-out while this is set: every new
    /// `SessionLoading` snapshot would otherwise replace the visible history
    /// with an empty skeleton and make a healthy session flash.
    session_load_in_flight: bool,
    pending_initial_session: Option<String>,
}

/// Accept only a load start newer than every generation this window has seen.
/// The daemon's generation is monotonic for the process, so this also fences a
/// late response from an earlier visit to the same session after a selection
/// change reset the active generation.
fn session_load_start_is_current(last_generation: Option<u64>, generation: u64) -> bool {
    last_generation.is_none_or(|last| generation > last)
}

/// Avoid overlapping the bounded panel fan-outs. Starting another load while
/// one is pending replaces the visible session with a loading skeleton and is
/// the source of the history/error flashing seen during slow restores.
fn should_poll_session(
    selected: bool,
    session_loading: bool,
    load_in_flight: bool,
    elapsed: Duration,
    interval: Duration,
) -> bool {
    selected && !session_loading && !load_in_flight && elapsed >= interval
}

/// Periodic refreshes update a populated session in place. Replacing it with
/// the empty loading snapshot is reserved for explicit selection/retry, where
/// the foreground loading state is intentional.
fn preserve_session_during_refresh(session_id: &str, session_loading: bool) -> bool {
    !session_loading && !session_id.is_empty()
}

impl PurrCodeIde {
    pub fn new(ctx: &Context, config: Config) -> Self {
        let appearance = Appearance::default();
        crate::theme::install(ctx, appearance);

        let repaint_ctx = ctx.clone();
        let client = DaemonClient::spawn(
            Connection::new(config.base_url.clone(), config.token.clone()),
            move || repaint_ctx.request_repaint(),
        );

        let mut recents = RecentWorkspaces::load();
        let (stage, repository) = match config.repository.clone() {
            Some(path) => {
                recents.record(&path);
                let _ = recents.save();
                (Stage::Workspace, path)
            }
            None => (Stage::Welcome, PathBuf::new()),
        };

        let now = Instant::now();
        let mut app = Self {
            client,
            connected: false,
            startup: Startup::Connecting,
            startup_began: now,
            last_transport_error: None,

            appearance,
            pending_appearance: appearance,
            tokens: Tokens::for_appearance(appearance),

            stage,
            repository,
            recents,
            welcome_error: None,
            workspace_state: WorkspaceView::default(),

            sessions: Vec::new(),
            selected: None,
            session: Session::default(),
            session_loading: false,
            submitting: false,

            composer: String::new(),
            pending_submission: None,
            focus_composer: true,
            ime_composing: false,

            activity: ActivityBar::Agent,
            sidebar_visible: true,
            agent_location: AgentLocation::EditorTab,
            agent_tab_dirty: false,
            aux_panel: Some(AuxView::Source),
            bottom_panel: None,
            dirty: BTreeSet::new(),
            composer_caret: 0,
            active_completion: None,
            completion_index: 0,
            commands: Vec::new(),
            workspace_symbols: Vec::new(),
            workspace_symbols_for: None,
            resolved_references: Vec::new(),
            references_requested_for: None,
            command_palette: None,
            quick_open: None,
            title_visible: false,

            expanded: BTreeSet::new(),
            search_query: String::new(),
            search_results: Vec::new(),
            search_ran: false,
            search_ran_for: None,

            code_panel: CodePanel::Source,
            open_files: Vec::new(),
            active_file: 0,
            pending_close: None,
            inline_diff_open: false,
            diff_scope: ChangeScope::Agent,
            diff: None,
            diff_loading: false,
            pending_clear_selected_diff: false,
            workspace_changes: model::Changes::default(),
            workspace_changes_loading: false,
            workspace_changes_checked: false,

            session_query: String::new(),
            session_hits: Vec::new(),
            session_hits_for: None,
            renaming_session: None,
            deleting_session: None,

            checkpoints: Vec::new(),
            checkpoints_for: None,
            pending_restore: None,

            language: crate::model::LanguageIntelligence::default(),
            hover_probe: None,
            caret: None,
            caret_word: String::new(),
            // On by default: a formatter that has to be remembered is a
            // formatter that produces noisy diffs. It is a no-op when no
            // language server covers the file.
            format_on_save: true,

            file_operation: None,
            file_operation_name: String::new(),
            file_operation_error: None,

            dock: None,

            terminals: Vec::new(),
            active_terminal: 0,
            focus_terminal: false,

            settings_open: false,
            settings_page: settings::SettingsPage::General,
            settings_search: String::new(),
            provider_base_url: "http://127.0.0.1:11434".to_owned(),
            provider_model: String::new(),
            provider_api_key: String::new(),
            discover_type: "ollama".to_owned(),
            diagnostics_open: false,
            models: Vec::new(),
            bootstrap: Value::Null,
            settings_state: settings::SettingsState::default(),
            provider_role: std::collections::BTreeMap::new(),
            pull_model_input: String::new(),
            local_policy: None,
            local_idle_timeout: None,
            skill_capability_input: String::new(),
            skill_publisher_input: String::new(),
            codex_binary: String::new(),
            codex_timeout: 3600,
            codex_inherit_auth: true,
            codex_require_diff: true,
            codex_allow_tree_write: false,
            mcp_id: String::new(),
            mcp_program: String::new(),
            mcp_arguments: String::new(),
            mcp_working_directory: String::new(),
            mcp_network: false,
            mcp_environment: String::new(),

            notices: Vec::new(),

            last_session_poll: now,
            last_list_poll: now,
            last_terminal_poll: now,
            last_workspace_changes_poll: now,
            last_diagnostic_poll: now,
            last_disk_check: now,
            session_loading_began: None,
            current_session_load_generation: None,
            last_session_load_generation: None,
            session_load_in_flight: false,
            pending_initial_session: config.initial_session,
        };

        app.client.send(Request::Bootstrap);
        app.client.send(Request::ListModels);
        // The palette and the composer's `/` completion are both driven by
        // the daemon's command contract, so fetch it once at startup rather
        // than hard-coding a list that would drift from the daemon's.
        app.client.send(Request::ListCommands);
        if app.stage == Stage::Workspace {
            app.refresh_workspace();
        }
        app
    }

    // ── Requests ───────────────────────────────────────────────────────

    pub(crate) fn repository_string(&self) -> String {
        self.repository.display().to_string()
    }

    pub(crate) fn repository_name(&self) -> String {
        self.repository
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.repository.display().to_string())
    }

    /// Ask for everything that describes the open folder.
    pub(crate) fn refresh_workspace(&mut self) {
        if self.stage != Stage::Workspace {
            return;
        }
        let repository = self.repository_string();
        self.client.send(Request::WorkspaceState {
            repository: repository.clone(),
        });
        self.client.send(Request::ListSessions { repository });
        self.client.send(Request::ListTerminals);
        self.request_workspace_changes();
    }

    /// Open Settings and fetch the page's own data (FR-A1).
    pub(crate) fn open_settings(&mut self) {
        self.settings_open = true;
        self.settings_refresh_current();
    }

    /// Re-fetch whatever the active settings page renders (FR-A1). Every page
    /// lists the queries it owns; nothing renders a value that was never
    /// fetched. Called when Settings opens, when the page changes, and after a
    /// mutation lands (`SettingsMutated`).
    pub(crate) fn settings_refresh_current(&mut self) {
        use crate::app::settings::SettingsPage;
        match self.settings_page {
            SettingsPage::General => {}
            SettingsPage::Models => {
                self.client.send(Request::ListProviders);
                self.client.send(Request::ListModels);
            }
            SettingsPage::LocalModels => {
                self.client.send(Request::LocalModelsStatus);
                self.client.send(Request::LocalModelsRecommendations);
                self.client.send(Request::LocalModelsGetSettings);
            }
            SettingsPage::Skills => {
                self.client.send(Request::ListSkills);
            }
            SettingsPage::Mcp => {
                self.client.send(Request::McpList);
            }
            SettingsPage::Codex => {
                self.client.send(Request::CodexGet);
            }
            SettingsPage::Authority
            | SettingsPage::Agent
            | SettingsPage::Terminal
            | SettingsPage::Privacy
            | SettingsPage::Advanced => {}
        }
    }

    /// Ask for per-file change counts for the open folder.
    ///
    /// The panel and status bar read the same change set, so the number of rows
    /// and the status bar's number agree (FR-C8). Polled at the workspace
    /// cadence, not the session cadence, because numstat over a dirty tree is
    /// not free (PRD §3.4).
    ///
    /// The scope is always the user's uncommitted working tree: a plain folder
    /// has no session base revision, so `agent` never applies here. A folder
    /// that is not a Git repository is not polled at all.
    pub(crate) fn request_workspace_changes(&mut self) {
        if self.stage != Stage::Workspace || !self.workspace_state.is_git_repository {
            return;
        }
        if self.workspace_changes_loading {
            return;
        }
        self.workspace_changes_loading = true;
        self.client.send(Request::WorkspaceChanges {
            repository: self.repository_string(),
            scope: ChangeScope::WorkingTree.slug(),
        });
    }

    pub(crate) fn reload_session(&mut self) {
        let Some(session) = self.selected.clone() else {
            return;
        };
        // Fence responses from the previous load before asking for a fresh
        // generation. The daemon tags every panel, so an old response can no
        // longer repopulate the screen between a mutation/retry and its new
        // loading snapshot.
        self.current_session_load_generation = None;
        self.session_load_in_flight = true;
        self.last_session_poll = Instant::now();
        self.client.send(Request::LoadSession {
            session,
            scope: self.diff_scope.slug(),
        });
    }

    pub(crate) fn select_session(&mut self, id: &str) {
        if self.selected.as_deref() == Some(id) {
            // A second click is an explicit retry. This matters for a panel
            // request that failed or for a quarantined legacy session whose
            // recovery status changed while the IDE stayed open.
            self.session_loading = true;
            self.session_loading_began = Some(Instant::now());
            self.current_session_load_generation = None;
            self.session_load_in_flight = false;
            self.reload_session();
            self.refresh_diff();
            return;
        }
        self.selected = Some(id.to_owned());
        self.session = Session::default();
        self.session_loading = true;
        self.session_loading_began = Some(Instant::now());
        self.current_session_load_generation = None;
        self.session_load_in_flight = false;
        self.diff = None;
        self.inline_diff_open = false;
        // The previously opened file's diff selection does not survive a
        // session switch; a stale path must not filter the next session's diff.
        self.clear_selected_diff();
        self.reload_session();
        self.refresh_diff();
    }

    pub(crate) fn refresh_diff(&mut self) {
        let Some(session) = self.selected.clone() else {
            self.diff = None;
            return;
        };
        self.diff_loading = true;
        self.client.send(Request::FileDiff {
            session,
            scope: self.diff_scope.slug(),
        });
    }

    /// Forget which file's diff is open. Called on session and workspace
    /// switches so a stale path never filters the next session's diff.
    pub(crate) fn clear_selected_diff(&mut self) {
        // The key lives in egui temp-data; the Context is not available here,
        // so the removal is deferred to the frame that has one.
        self.pending_clear_selected_diff = true;
    }

    /// Fetch the diff for a specific change-set scope, so a click on a
    /// workspace (working-tree) file shows that file's working-tree diff rather
    /// than the agent diff. Falls back to the plain [`Self::refresh_diff`]
    /// scope for any scope the transport cannot address.
    pub(crate) fn refresh_diff_for_scope(&mut self, scope: ChangeScope) {
        let Some(session) = self.selected.clone() else {
            self.diff = None;
            return;
        };
        self.diff_loading = true;
        self.client.send(Request::FileDiff {
            session,
            scope: scope.slug(),
        });
    }

    /// Send whatever the user typed.
    ///
    /// This is the whole of PRD §9: one composer, one action, no mode. The
    /// task mode travels as `auto` so the daemon resolves the workflow from
    /// the request itself (PRD §5, §7, FR-002, FR-004) — a greeting stays a
    /// greeting and never creates a worktree, a plan or a validation run.
    pub(crate) fn submit(&mut self) {
        let text = self.composer.trim().to_owned();
        if text.is_empty() || self.submitting {
            return;
        }
        if !self.connected {
            self.push_notice(errors::disconnected_notice());
            return;
        }
        self.submitting = true;
        self.pending_submission = Some(text.clone());
        match self.selected.clone() {
            Some(session) => self.client.send(Request::SendMessage {
                session,
                content: text,
            }),
            None => self.client.send(Request::StartSession {
                objective: text,
                repository: self.repository_string(),
                // No override: the daemon routes to a configured model
                // (PRD §27). An override lives in Settings, not here.
                model: None,
                // The daemon classifies intent and complexity. A greeting
                // stays a greeting; a risky build can still produce a plan.
                task_mode: "auto".to_owned(),
                execution_style: "autonomous".to_owned(),
                permission_mode: "auto".to_owned(),
                plan_only: false,
            }),
        }
        self.composer.clear();
        self.focus_composer = true;
    }

    pub(crate) fn open_workspace(&mut self, path: PathBuf) {
        if !path.is_dir() {
            self.welcome_error = Some(format!(
                "{} is not a folder PurrCode can open.",
                path.display()
            ));
            return;
        }
        self.recents.record(&path);
        let _ = self.recents.save();
        self.repository = path;
        self.stage = Stage::Workspace;
        self.welcome_error = None;
        self.sessions.clear();
        self.selected = None;
        self.session = Session::default();
        self.current_session_load_generation = None;
        self.session_load_in_flight = false;
        self.open_files.clear();
        self.expanded.clear();
        self.diff = None;
        self.search_results.clear();
        self.search_ran = false;
        self.search_ran_for = None;
        self.aux_panel = Some(AuxView::Source);
        self.agent_location = AgentLocation::EditorTab;
        self.code_panel = CodePanel::Source;
        self.inline_diff_open = false;
        // A new folder owns fresh change state: the previous folder's change
        // set, checked flag and poll timer must not leak into it (the Source
        // Control panel and status bar would show the old folder's files under
        // the new one until the first check lands).
        self.workspace_changes = model::Changes::default();
        self.workspace_changes_checked = false;
        self.workspace_changes_loading = false;
        self.last_workspace_changes_poll = Instant::now();
        self.clear_selected_diff();
        self.refresh_workspace();
    }

    pub(crate) fn close_workspace(&mut self) {
        if let Some(picked) = crate::welcome::pick_folder(Some(&self.repository)) {
            self.open_workspace(picked);
        }
    }

    pub(crate) fn push_notice(&mut self, notice: Notice) {
        if self
            .notices
            .iter()
            .any(|existing| existing.headline == notice.headline)
        {
            return;
        }
        self.notices.push(notice);
    }

    pub(crate) fn dock_badge(&self, tab: DockTab) -> Option<String> {
        match tab {
            DockTab::Tests => {
                if self.session.validation.stages.is_empty() {
                    None
                } else {
                    let failed = self.session.validation.failed();
                    if failed > 0 {
                        Some(failed.to_string())
                    } else if self.session.validation.complete {
                        Some("✓".to_owned())
                    } else {
                        Some("…".to_owned())
                    }
                }
            }
            DockTab::Problems => {
                // The badge counts both sources the panel shows. Counting only
                // validation would leave a file full of compile errors badged
                // as clean while the panel below listed every one of them.
                let (errors, others) = self.language.counts();
                let total =
                    crate::model::problems_from(&self.session.validation).len() + errors + others;
                (total > 0).then(|| total.to_string())
            }
            DockTab::Terminal => {
                if self.terminals.is_empty() {
                    None
                } else {
                    Some(self.terminals.len().to_string())
                }
            }
            DockTab::Activity => {
                if self.session.activity.is_empty() {
                    None
                } else {
                    Some("•".to_owned())
                }
            }
        }
    }

    // ── Responses ──────────────────────────────────────────────────────

    fn drain(&mut self) {
        for response in self.client.drain() {
            self.handle(response);
        }
    }

    fn handle(&mut self, response: Response) {
        match response {
            Response::Connected(connected) => {
                let was = self.connected;
                self.connected = connected;
                if connected {
                    self.last_transport_error = None;
                    self.notices.retain(|notice| !notice.clears_on_reconnect);
                    if self.startup != Startup::Ready {
                        self.startup = Startup::Ready;
                    }
                } else if was {
                    self.push_notice(errors::disconnected_notice());
                }
            }
            Response::Bootstrap(value) => {
                self.bootstrap = value;
                self.connected = true;
                self.startup = if self.stage == Stage::Workspace && self.sessions.is_empty() {
                    Startup::Restoring
                } else {
                    Startup::Ready
                };
            }
            Response::Sessions(rows) => {
                self.sessions = model::parse_session_rows_for(&rows, &self.repository);
                self.startup = Startup::Ready;
                // A user can press New session while the explicit launcher
                // session is still loading. Do not let a later list refresh
                // resurrect that pending automatic selection.
                if self.selected.is_none() && self.session_load_in_flight {
                    self.pending_initial_session = None;
                }
                if self.selected.is_none()
                    && !self.session_load_in_flight
                    && let Some(wanted) = self.pending_initial_session.take()
                    && self.sessions.iter().any(|row| row.id == wanted)
                {
                    self.select_session(&wanted);
                }
            }
            Response::Session(id, snapshot) => {
                // Legacy untagged snapshots must not win over a generation
                // already in flight. The bounded loader is the only current
                // path, but keeping this fence makes a delayed response from
                // an older daemon harmless during upgrades.
                if self.selected.as_deref() == Some(id.as_str()) && !self.session_load_in_flight {
                    self.session = model::parse_session(&id, &snapshot);
                    self.session_loading = false;
                    self.session_loading_began = None;
                    self.submitting = false;
                    self.surface_panel_failures();
                }
            }
            Response::SessionLoading(id, generation, snapshot) => {
                if self.selected.as_deref() == Some(id.as_str())
                    && session_load_start_is_current(self.last_session_load_generation, generation)
                {
                    let preserve =
                        preserve_session_during_refresh(&self.session.id, self.session_loading);
                    if !preserve {
                        self.session = model::parse_session(&id, &snapshot);
                    }
                    self.current_session_load_generation = Some(generation);
                    self.last_session_load_generation = Some(generation);
                    self.session_load_in_flight = true;
                    if !preserve {
                        self.session_loading = true;
                        self.session_loading_began = Some(Instant::now());
                    }
                }
            }
            Response::Panel(id, kind, result) => {
                if self.selected.as_deref() == Some(id.as_str()) {
                    model::apply_panel(&mut self.session, kind, result);
                    // A snapshot is complete when every independently fetched
                    // panel has reached a terminal availability. This avoids
                    // the old fixed eight-second wait after a healthy history
                    // click while keeping loading/empty/unavailable distinct.
                    let complete = PanelKind::ALL.iter().all(|panel| {
                        self.session.panel_states.get(panel).is_some_and(|state| {
                            state.availability != crate::daemon::PanelAvailability::Loading
                        })
                    });
                    if complete {
                        self.session_loading = false;
                        self.session_loading_began = None;
                        self.surface_panel_failures();
                    }
                }
            }
            Response::SessionPanel(id, generation, kind, result) => {
                if self.selected.as_deref() == Some(id.as_str())
                    && self.current_session_load_generation == Some(generation)
                {
                    model::apply_panel(&mut self.session, kind, result);
                }
            }
            Response::SessionLoaded(id, generation) => {
                if self.selected.as_deref() == Some(id.as_str())
                    && self.current_session_load_generation == Some(generation)
                {
                    self.session_loading = false;
                    self.session_loading_began = None;
                    self.session_load_in_flight = false;
                    self.surface_panel_failures();
                }
            }
            Response::SessionStarted(id) => {
                self.submitting = false;
                self.pending_submission = None;
                self.selected = Some(id.clone());
                self.session = Session::default();
                self.session_loading = true;
                self.session_loading_began = Some(Instant::now());
                self.current_session_load_generation = None;
                self.session_load_in_flight = false;
                self.reload_session();
                self.refresh_workspace();
            }
            Response::Mutated(id) => {
                self.submitting = false;
                self.pending_submission = None;
                if self.selected.as_deref() == Some(id.as_str()) {
                    self.reload_session();
                    self.refresh_diff();
                }
                let repository = self.repository_string();
                self.client.send(Request::ListSessions { repository });
            }
            Response::Diff(id, patch) => {
                if self.selected.as_deref() == Some(id.as_str()) {
                    self.diff = Some(patch);
                    self.diff_loading = false;
                }
            }
            Response::Hunks(_, _) => {}
            Response::Models(values) => {
                self.models = values
                    .iter()
                    .filter_map(|value| {
                        Some(ModelChoice {
                            id: value.get("id")?.as_str()?.to_owned(),
                            provider: value
                                .get("provider")
                                .and_then(Value::as_str)
                                .unwrap_or("")
                                .to_owned(),
                            is_default: value
                                .get("default")
                                .and_then(Value::as_bool)
                                .unwrap_or(false),
                            local: value.get("local").and_then(Value::as_bool).unwrap_or(false),
                            roles: value
                                .get("roles")
                                .and_then(Value::as_array)
                                .map(|roles| {
                                    roles
                                        .iter()
                                        .filter_map(Value::as_str)
                                        .map(str::to_owned)
                                        .collect()
                                })
                                .unwrap_or_default(),
                        })
                    })
                    .collect();
            }
            Response::TerminalStarted(value) => self.terminal_started(value),
            Response::TerminalOutput(value) => self.terminal_output(value),
            Response::TerminalChanged(value) => self.terminal_changed(value),
            Response::Terminals(values) => self.terminals_listed(serde_json::Value::Array(values)),
            Response::Workspace(value) => {
                self.workspace_state = WorkspaceView::parse(&value);
            }
            Response::WorkspaceChanges(value) => {
                self.workspace_changes = model::parse_changes(&value);
                self.workspace_changes_loading = false;
                self.workspace_changes_checked = true;
            }
            // ── Settings surfaces (Defect A) ───────────────────────────
            Response::Providers(values) => self.settings_state.providers = values,
            Response::Provider(value) => self.settings_state.provider = value,
            Response::ProviderTested(value) => self.settings_state.provider_test = value,
            Response::DiscoveredModels(value) => self.settings_state.discovered = value,
            Response::LocalModels(value) => self.settings_state.local_models = value,
            Response::LocalModelsRecommendations(value) => {
                self.settings_state.recommendations = value
            }
            Response::LocalModelsQualified(value) => self.settings_state.qualification = value,
            Response::LocalModelsUnloaded(value) => {
                self.settings_state.unload = value;
                self.client.send(Request::LocalModelsStatus);
            }
            Response::LocalModelsPullProposed(value) => {
                self.settings_state.pull_proposal = value.clone();
                if let Some(id) = value
                    .get("action_id")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
                {
                    self.settings_state.pull_action_id = Some(id);
                }
                if let Some(session) = value
                    .get("session_id")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
                {
                    self.settings_state.pull_session_id = Some(session);
                }
            }
            Response::LocalModelsPullApproved(value) => {
                self.settings_state.pull_approved = value;
                self.settings_state.pull_progress = Value::Null;
            }
            Response::LocalModelsPullStarted(value) => {
                self.settings_state.pull_started = value.clone();
                self.settings_state.pull_progress = value;
            }
            Response::LocalModelsPullProgress(value) => {
                let terminal = value
                    .get("phase")
                    .and_then(Value::as_str)
                    .is_some_and(|phase| matches!(phase, "completed" | "cancelled" | "failed"));
                self.settings_state.pull_progress = value;
                if terminal {
                    self.client.send(Request::LocalModelsStatus);
                }
            }
            Response::LocalModelsPullCancelled(value) => {
                self.settings_state.pull_cancelled = value;
                self.client.send(Request::LocalModelsStatus);
            }
            Response::LocalModelsSettings(value) => self.settings_state.local_settings = value,
            Response::Skills(values) => self.settings_state.skills = values,
            Response::Skill(value) => self.settings_state.skill = value,
            Response::SkillRemoved(value) => {
                self.settings_state.skill_removed = value;
                self.settings_state.mutation_succeeded();
            }
            Response::SkillSearch(value) => {
                // The first answer may carry an approval action_id; the
                // resolved answer is an array of candidates.
                if let Some(id) = value
                    .get("action_id")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
                {
                    self.settings_state.skill_search_action_id = Some(id);
                }
                self.settings_state.skill_search = value;
            }
            Response::SkillDownloaded(value) => {
                if let Some(id) = value
                    .get("action_id")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
                {
                    self.settings_state.skill_download_action_id = Some(id);
                }
                self.settings_state.skill_downloaded = value;
            }
            Response::SkillInstallProposed(value) => {
                if let Some(id) = value
                    .get("action_id")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
                {
                    self.settings_state.skill_install_action_id = Some(id);
                }
                self.settings_state.skill_install = value;
            }
            Response::SkillInstallApproved(value) => {
                self.settings_state.skill_install = value;
            }
            Response::SkillInstalled(value) => {
                self.settings_state.skill_install = value;
                self.settings_state.skill_install_action_id = None;
                self.client.send(Request::ListSkills);
            }
            Response::SkillPublisherBlocked(value) => {
                self.settings_state.skill_publisher_blocked = value;
                self.settings_state.mutation_succeeded();
            }
            Response::McpServers(value) => self.settings_state.mcp_servers = value,
            Response::McpServerSaved(value) => {
                self.settings_state.mcp_saved = value;
                self.client.send(Request::McpList);
            }
            Response::McpServerRemoved(value) => {
                self.settings_state.mcp_removed = value;
                self.client.send(Request::McpList);
            }
            Response::McpProbed(value) => self.settings_state.mcp_probe = value,
            Response::Codex(value) => self.settings_state.codex = value,
            Response::CodexSaved(value) => {
                self.settings_state.codex_saved = value.clone();
                self.settings_state.codex = value;
            }
            Response::CodexDoctor(value) => self.settings_state.codex_doctor = value,
            Response::LspServers(value) => {
                self.language.servers = value["servers"]
                    .as_array()
                    .map(|items| {
                        items
                            .iter()
                            .filter_map(|item| {
                                Some((
                                    item["program"].as_str()?.to_owned(),
                                    item["extensions"]
                                        .as_array()?
                                        .iter()
                                        .filter_map(|ext| Some(ext.as_str()?.to_ascii_lowercase()))
                                        .collect(),
                                ))
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                self.language.servers_checked = true;
                self.language.last_error = None;
                // The servers are known now, so anything already open can be
                // handed over for analysis.
                for path in self
                    .open_files
                    .iter()
                    .map(|file| file.path.clone())
                    .collect::<Vec<_>>()
                {
                    self.open_in_language_server(&path);
                }
            }
            Response::LspHover(path, line, character, value) => {
                let anchor = crate::model::DocumentPosition { line, character };
                // Only show an answer for the position the pointer is still
                // resting on: a reply that arrives after the pointer moved
                // would label the wrong token.
                let current = self
                    .hover_probe
                    .as_ref()
                    .is_some_and(|probe| probe.path == path && probe.position == anchor);
                if current {
                    let contents = value["contents"].as_str().unwrap_or_default().trim();
                    self.language.hover = (!contents.is_empty()).then(|| contents.to_owned());
                    self.language.hover_anchor = Some((path, anchor));
                }
            }
            Response::LspDefinition(value) => {
                let targets: Vec<crate::model::Location> = value
                    .as_array()
                    .map(|items| {
                        items
                            .iter()
                            .filter_map(crate::model::Location::parse)
                            .collect()
                    })
                    .unwrap_or_default();
                match targets.first() {
                    // One target is an unambiguous jump. Several means the
                    // symbol genuinely has multiple definitions (trait impls,
                    // overloads), so the list is shown rather than guessing.
                    Some(target) if targets.len() == 1 => self.open_location(target),
                    Some(_) => {
                        self.language.references = targets;
                        self.language.references_checked = true;
                        self.language.references_for = Some("definitions".to_owned());
                        self.aux_panel = Some(AuxView::References);
                    }
                    None => self.push_notice(errors::Notice {
                        kind: errors::NoticeKind::Environment,
                        headline: "No definition found".to_owned(),
                        impact: "The language server did not resolve a definition for that symbol."
                            .to_owned(),
                        next_step: Some(
                            "It may still be indexing the project — try again in a moment."
                                .to_owned(),
                        ),
                        actions: vec![errors::NoticeAction::Dismiss],
                        detail: None,
                        clears_on_reconnect: false,
                    }),
                }
            }
            Response::LspReferences(label, value) => {
                self.language.references = value
                    .as_array()
                    .map(|items| {
                        items
                            .iter()
                            .filter_map(crate::model::Location::parse)
                            .collect()
                    })
                    .unwrap_or_default();
                self.language.references_checked = true;
                self.language.references_for = Some(label);
            }
            Response::LspSymbols(path, value) => {
                self.language.symbols = value
                    .as_array()
                    .map(|items| {
                        items
                            .iter()
                            .filter_map(crate::model::Symbol::parse)
                            .collect()
                    })
                    .unwrap_or_default();
                self.language.symbols_for = Some(path);
            }
            Response::LspFormat(path, value, then_save) => {
                self.apply_format_edits(&path, &value, then_save)
            }
            Response::LspDiagnostics(value) => self.language.absorb_diagnostics(&value),
            Response::SessionSearch(query, value) => {
                if self.session_hits_for.as_deref() == Some(query.as_str()) {
                    self.session_hits = crate::model::SessionHit::parse_all(&value);
                }
            }
            Response::Checkpoints(session, value) => {
                if self.selected.as_deref() == Some(session.as_str()) {
                    self.checkpoints = crate::model::Checkpoint::parse_all(&value);
                }
            }
            Response::CheckpointPreview(checkpoint, value) => {
                let files: Vec<String> = value["changed_files"]
                    .as_array()
                    .map(|items| {
                        items
                            .iter()
                            .filter_map(|item| item.as_str().map(str::to_owned))
                            .collect()
                    })
                    .unwrap_or_default();
                // Fold the preview into both the list entry and the pending
                // dialog, so the confirmation button unlocks with a real
                // number behind it.
                for entry in &mut self.checkpoints {
                    if entry.id == checkpoint {
                        entry.changed_files = Some(files.clone());
                    }
                }
                if let Some(pending) = self.pending_restore.as_mut()
                    && pending.checkpoint.id == checkpoint
                {
                    pending.checkpoint.changed_files = Some(files);
                }
            }
            Response::CheckpointRestored(_) => {
                // The worktree changed underneath every open buffer, so make
                // the editor re-read rather than letting a stale buffer be
                // saved back over the restored file.
                for file in &mut self.open_files {
                    file.disk_stamp = None;
                }
                self.last_disk_check = Instant::now() - crate::app::language::DISK_CHECK;
                self.checkpoints_for = None;
            }
            Response::SessionForked(child) => {
                if child.is_empty() {
                    return;
                }
                // Select the fork: the user asked to try another approach, and
                // leaving them in the parent means the next thing they type
                // lands in the session they were trying to branch away from.
                self.pending_initial_session = Some(child.clone());
                self.select_session(&child);
                let repository = self.repository_string();
                self.client.send(Request::ListSessions { repository });
            }
            Response::LspWorkspaceSymbols(query, value) => {
                // Only accept the answer to the query still being typed.
                if self.workspace_symbols_for.as_deref() == Some(query.as_str()) {
                    self.workspace_symbols = value
                        .as_array()
                        .map(|items| {
                            items
                                .iter()
                                .filter_map(|item| {
                                    let symbol = crate::model::Symbol::parse(&serde_json::json!({
                                        "name": item["name"],
                                        "kind": item["kind"],
                                    }))?;
                                    Some((
                                        symbol.name.clone(),
                                        symbol.kind_label().to_owned(),
                                        item["container"].as_str().map(str::to_owned),
                                    ))
                                })
                                .collect()
                        })
                        .unwrap_or_default();
                }
            }
            Response::Commands(value) => {
                self.commands = value
                    .as_array()
                    .map(|items| {
                        items
                            .iter()
                            .filter_map(|item| {
                                Some((
                                    item["name"].as_str()?.to_owned(),
                                    item["description"].as_str().unwrap_or_default().to_owned(),
                                    item["group"].as_str().unwrap_or_default().to_owned(),
                                ))
                            })
                            .collect()
                    })
                    .unwrap_or_default();
            }
            Response::References(text, value) => {
                // The draft may have moved on while this was in flight; a
                // chip row describing text the user has edited past would
                // claim the wrong files are attached.
                if self.composer == text {
                    self.resolved_references = crate::model::ResolvedReference::parse_all(&value);
                }
            }
            Response::LspUnavailable(error) => {
                // Recorded, not raised: a missing language server is a normal
                // state for a machine, and a modal per hover would be worse
                // than no intelligence at all.
                self.language.last_error = Some(error);
            }
            Response::SettingsMutated => {
                // A mutation landed; the owning page re-fetches its data.
                self.settings_state.mutation_succeeded();
                self.settings_refresh_current();
            }
            Response::SubmissionFailed(error) => {
                self.last_transport_error = Some(error.clone());
                if self.composer.trim().is_empty()
                    && let Some(draft) = self.pending_submission.take()
                {
                    self.composer = draft;
                    self.focus_composer = true;
                }
                self.submitting = false;
                if self.stage == Stage::Workspace {
                    self.client.send(Request::ListSessions {
                        repository: self.repository_string(),
                    });
                }
                if let Some(notice) = errors::translate(&error) {
                    self.push_notice(notice);
                }
            }
            Response::Failed(error) => {
                self.last_transport_error = Some(error.clone());
                self.session_loading = false;
                self.session_loading_began = None;
                self.diff_loading = false;
                self.workspace_changes_loading = false;
                // FR-A8: a failed settings mutation is attributed to the
                // control that produced it so it renders inline, never as a
                // silent no-op.
                if let Some(key) = self.settings_state.pending.clone() {
                    self.settings_state.errors.insert(key, error.clone());
                    self.settings_state.pending = None;
                }
                if self.startup == Startup::Connecting
                    && self.startup_began.elapsed() > Duration::from_secs(6)
                {
                    self.startup = Startup::Unavailable;
                }
                if let Some(notice) = errors::translate(&error) {
                    self.push_notice(notice);
                }
            }
        }
    }

    /// Turn a failed panel into one honest sentence rather than an empty box.
    fn surface_panel_failures(&mut self) {
        let failures: Vec<Notice> = PanelKind::ALL
            .iter()
            .filter_map(|kind| {
                let panel = self.session.panel(*kind);
                let error = panel.error.clone()?;
                Some(errors::panel_notice(*kind, &error))
            })
            .collect();
        for notice in failures {
            self.push_notice(notice);
        }
    }

    // ── Frame ──────────────────────────────────────────────────────────

    fn tick(&mut self, ctx: &Context) {
        // A session load that never resolves (e.g. a saturated panel queue)
        // must not spin the "Loading…" indicator forever. Fall back after a
        // few seconds and surface whatever panels did come back.
        if self.session_loading
            && self
                .session_loading_began
                .is_some_and(|began| began.elapsed() > Duration::from_secs(8))
        {
            self.session_loading = false;
            self.session_loading_began = None;
            // The generation has timed out, so release the poll fence and
            // discard its active correlation. Any panels that arrive later
            // are stale until a fresh SessionLoading generation is accepted.
            self.session_load_in_flight = false;
            self.current_session_load_generation = None;
            self.surface_panel_failures();
        }
        let running = self.session.state_view().execution_active;
        let session_interval = if running {
            Duration::from_millis(700)
        } else {
            Duration::from_millis(2500)
        };
        if should_poll_session(
            self.selected.is_some(),
            self.session_loading,
            self.session_load_in_flight,
            self.last_session_poll.elapsed(),
            session_interval,
        ) {
            self.last_session_poll = Instant::now();
            self.reload_session();
        }
        if self.stage == Stage::Workspace && self.last_list_poll.elapsed() >= Duration::from_secs(4)
        {
            self.last_list_poll = Instant::now();
            let repository = self.repository_string();
            self.client.send(Request::ListSessions { repository });
        }
        // Workspace change counts ride the same 4 s workspace cadence as the
        // session list, never the 700 ms session cadence: numstat over a dirty
        // tree is the cost PRD §3.4 says to avoid paying per keystroke.
        if self.stage == Stage::Workspace
            && self.last_workspace_changes_poll.elapsed() >= Duration::from_secs(4)
        {
            self.last_workspace_changes_poll = Instant::now();
            self.request_workspace_changes();
        }
        if !self.terminals.is_empty()
            && self.last_terminal_poll.elapsed() >= Duration::from_millis(120)
        {
            self.last_terminal_poll = Instant::now();
            self.poll_terminals();
        }
        // Language intelligence: find out what is installed once, then keep
        // the Problems panel fed. Both are no-ops when no server exists.
        if self.stage == Stage::Workspace {
            self.probe_language_servers();
            self.poll_diagnostics();
            self.settle_hover();
            self.detect_external_changes();
            self.refresh_checkpoints();
        }
        // A running task should look running without the user touching the
        // mouse; an idle window should not burn a core.
        ctx.request_repaint_after(if running || !self.terminals.is_empty() {
            Duration::from_millis(120)
        } else {
            Duration::from_millis(600)
        });
    }
}

impl eframe::App for PurrCodeIde {
    fn update(&mut self, ctx: &Context, _frame: &mut eframe::Frame) {
        self.drain();
        self.tick(ctx);
        self.handle_shortcuts(ctx);
        if self.pending_clear_selected_diff {
            self.pending_clear_selected_diff = false;
            ctx.data_mut(|data| {
                data.remove::<String>(egui::Id::new(crate::app::code::SELECTED_DIFF_ID));
            });
        }

        // ── 1. Command bar ─────────────────────────────────────────────
        //
        // Panels carry no border of their own: egui already draws a hairline
        // between neighbours in `noninteractive.bg_stroke`, and a second box
        // around every region is what turns a workbench into a spreadsheet.
        //
        // The title bar is chrome, so it is raised a step above the canvas and
        // closed by a bottom hairline — the edge where it meets the work — so
        // the strip reads as its own surface and not as part of the canvas.
        egui::TopBottomPanel::top("purrcode_title_bar")
            .exact_height(theme::TITLE_BAR_HEIGHT)
            .frame(
                egui::Frame::new()
                    .fill(self.tokens.background_raised)
                    .inner_margin(egui::Margin::symmetric(8, 0)),
            )
            .show(ctx, |ui| {
                self.application_bar(ui);
                ui.painter().hline(
                    ui.max_rect().x_range(),
                    ui.max_rect().bottom(),
                    self.tokens.hairline(),
                );
            });

        // ── 2. Activity Bar (vertical icon rail) ───────────────────────
        egui::SidePanel::left("purrcode_activity_bar")
            .exact_width(theme::RAIL_WIDTH)
            .resizable(false)
            .show_separator_line(false)
            .frame(
                egui::Frame::new()
                    .fill(self.tokens.background_secondary)
                    .inner_margin(egui::Margin::same(0)),
            )
            .show(ctx, |ui| self.activity_bar(ui));

        // ── 3. Primary sidebar (Agent / Explorer / Search / Source) ─────
        if self.sidebar_visible {
            egui::SidePanel::left("purrcode_sidebar")
                .default_width(268.0)
                .width_range(200.0..=420.0)
                .resizable(true)
                .frame(
                    egui::Frame::new()
                        .fill(self.tokens.background_secondary)
                        .inner_margin(egui::Margin {
                            left: 8,
                            right: 6,
                            top: 8,
                            bottom: 0,
                        }),
                )
                .show(ctx, |ui| self.sidebar_panel(ui));
        }

        // ── 4. Right auxiliary panel (Agent / Changes / Source) ────────
        if let Some(view) = self.aux_panel {
            egui::SidePanel::right("purrcode_aux")
                .default_width(400.0)
                .width_range(320.0..=720.0)
                .resizable(true)
                .frame(
                    egui::Frame::new()
                        .fill(self.tokens.background_primary)
                        .inner_margin(egui::Margin::same(0)),
                )
                .show(ctx, |ui| self.aux_panel(ui, view));
        }

        // ── 5. Bottom panel (Terminal / Problems / Tests / Output) ─────
        if let Some(kind) = self.bottom_panel {
            egui::TopBottomPanel::bottom("purrcode_bottom_panel")
                .default_height(240.0)
                .height_range(120.0..=560.0)
                .resizable(true)
                .frame(
                    egui::Frame::new()
                        .fill(self.tokens.background_primary)
                        .inner_margin(egui::Margin {
                            left: 12,
                            right: 12,
                            top: 8,
                            bottom: 8,
                        }),
                )
                .show(ctx, |ui| self.bottom_panel(ui, kind));
        }

        // ── Status bar (always at the very bottom) ─────────────────────
        //
        // The raised fill and the top hairline (the edge where the bar meets
        // the canvas) are drawn inside `status_bar` so both bars own their own
        // surface; the frame here matches so there is never a flash of the
        // chrome colour before the raised fill lands.
        egui::TopBottomPanel::bottom("purrcode_status_bar")
            .exact_height(theme::STATUS_BAR_HEIGHT)
            .show_separator_line(false)
            .frame(
                egui::Frame::new()
                    .fill(self.tokens.background_raised)
                    .inner_margin(egui::Margin::symmetric(10, 0)),
            )
            .show(ctx, |ui| self.status_bar(ui));

        // ── 6. Editor / Agent center ──────────────────────────────────
        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(self.tokens.background_primary)
                    .inner_margin(egui::Margin::same(0)),
            )
            .show(ctx, |ui| match self.stage {
                Stage::Welcome => {
                    let choice = crate::welcome::pane(
                        ui,
                        &self.tokens,
                        &self.recents,
                        self.welcome_error.as_deref(),
                    );
                    self.apply_welcome_choice(choice);
                }
                Stage::Workspace => self.editor_center(ui),
            });

        // Command center overlays (Cmd+P / Cmd+Shift+P) draw last.
        self.command_center(ctx);

        self.settings_window(ctx);

        self.file_operation_dialog(ctx);

        self.restore_dialog(ctx);

        self.session_dialogs(ctx);

        self.close_confirm_modal(ctx);
    }
}

impl PurrCodeIde {
    pub(crate) fn apply_welcome_choice(&mut self, choice: Option<WelcomeChoice>) {
        match choice {
            Some(WelcomeChoice::Open(path)) => self.open_workspace(path),
            Some(WelcomeChoice::Browse) => {
                if let Some(picked) = crate::welcome::pick_folder(None) {
                    self.open_workspace(picked);
                }
            }
            Some(WelcomeChoice::Forget(path)) => {
                self.recents.forget(&path);
                let _ = self.recents.save();
            }
            None => {}
        }
    }

    /// A muted heading, used by every column so they read as one product.
    ///
    /// Small, upper-case and quiet: a panel title is a label for the region
    /// under it, never a competitor to the content it names.
    pub(crate) fn section_heading(&self, ui: &mut Ui, text: &str) {
        self.section_heading_label(ui, text);
        ui.add_space(6.0);
    }

    /// The label alone, for a caller inside a `horizontal` row where the
    /// trailing `add_space` would become horizontal padding instead of air
    /// below the heading.
    pub(crate) fn section_heading_label(&self, ui: &mut Ui, text: &str) {
        ui.label(
            RichText::new(text.to_ascii_uppercase())
                .size(theme::TYPE_EYEBROW)
                .strong()
                .color(self.tokens.text_muted),
        );
    }

    /// The hover fill every ghost button in the chrome shares.
    pub(crate) fn hover_fill(&self) -> egui::Color32 {
        self.tokens.surface_hover
    }

    /// A ghost icon button sized for the chrome.
    pub(crate) fn icon_action(
        &self,
        ui: &mut Ui,
        glyph: crate::icons::Glyph,
        tooltip: &str,
    ) -> egui::Response {
        crate::icons::ghost_button(
            ui,
            glyph,
            24.0,
            14.0,
            self.tokens.text_muted,
            self.hover_fill(),
            tooltip,
        )
    }

    /// The connection indicator is a tiny cat-eye: the product's one piece of
    /// signature chrome also carries a real state. It never shows a URL, and
    /// its tooltip is a sentence (PRD §17, §18).
    pub(crate) fn connection_dot(&mut self, ui: &mut Ui) {
        let (color, text) = if self.connected {
            (
                self.tokens.status_success,
                "PurrCode's local service is running",
            )
        } else {
            (
                self.tokens.status_error,
                "PurrCode can't reach its local service. Your project files were not affected.",
            )
        };
        let (rect, response) = ui.allocate_exact_size(egui::vec2(14.0, 12.0), Sense::click());
        let center = rect.center();
        let painter = ui.painter();
        painter.circle_filled(center, 5.5, self.tokens.tint(color));
        painter.circle_stroke(center, 4.0, egui::Stroke::new(1.2_f32, color));
        painter.circle_filled(center, 2.3, color);
        painter.line_segment(
            [
                egui::pos2(center.x, center.y - 2.0),
                egui::pos2(center.x, center.y + 2.0),
            ],
            egui::Stroke::new(1.2_f32, self.tokens.background_secondary),
        );
        if response.clicked() {
            self.diagnostics_open = true;
        }
        response.on_hover_text(text);
    }

    /// The vertical Activity Bar rail (PRD §9-13). Each icon selects what the
    /// primary sidebar shows; the active item gets an accent tile.
    ///
    /// Icon-only, with the name and its shortcut in the tooltip. The previous
    /// version drew each glyph into `available_rect_before_wrap()` — the whole
    /// remaining column — which is why the mascot came out the size of the
    /// rail. Every item now gets an exact tile, so the rail is a rail.
    fn activity_bar(&mut self, ui: &mut Ui) {
        ui.spacing_mut().item_spacing = egui::vec2(0.0, 2.0);
        ui.add_space(8.0);

        ui.vertical_centered(|ui| {
            let (rect, response) = ui.allocate_exact_size(
                egui::vec2(theme::RAIL_ITEM, theme::RAIL_ITEM),
                Sense::click(),
            );
            // Keep the supplied cat mark itself visible on the rail. Hover
            // and the open-settings state still get a control-sized tint, but
            // it is a neutral surface: a blue square behind the raster logo
            // reads as a second badge around the badge.
            let fill = if self.settings_open {
                Some(self.tokens.surface_active)
            } else if response.hovered() {
                Some(self.tokens.surface_hover)
            } else {
                None
            };
            if let Some(fill) = fill {
                let tile = egui::Rect::from_center_size(
                    rect.center(),
                    egui::Vec2::splat(theme::RAIL_ITEM * 0.88),
                );
                ui.painter().rect_filled(tile, theme::RADIUS_CONTROL, fill);
            }
            crate::icons::brand_badge_at(ui, rect);
            if response.hovered() {
                ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
            }
            if response.on_hover_text("PurrCode").clicked() {
                self.open_settings();
            }
        });
        ui.add_space(10.0);

        let mut clicked: Option<ActivityBar> = None;
        for item in ActivityBar::ALL {
            let selected = self.activity == *item && self.sidebar_visible;
            let response = crate::icons::rail_item(
                ui,
                item.glyph(),
                selected,
                &self.tokens,
                &format!("{}  ⌘{}", item.label(), item.shortcut()),
            );
            if response.clicked() {
                clicked = Some(*item);
            }
        }
        if let Some(item) = clicked {
            // Clicking the item you are already on collapses the sidebar, the
            // way every editor with a rail behaves.
            if self.activity == item && self.sidebar_visible {
                self.sidebar_visible = false;
            } else {
                self.activity = item;
                self.sidebar_visible = true;
            }
        }

        // Settings anchors the bottom of the rail, away from the navigation it
        // is not part of.
        ui.with_layout(Layout::bottom_up(Align::Center), |ui| {
            ui.add_space(8.0);
            if crate::icons::ghost_button(
                ui,
                crate::icons::Glyph::Gear,
                theme::RAIL_ITEM,
                17.0,
                self.tokens.text_muted,
                self.tokens.surface_hover,
                "Settings",
            )
            .clicked()
            {
                self.open_settings();
            }
        });
    }

    /// The primary sidebar: content depends on the selected Activity Bar item.
    fn sidebar_panel(&mut self, ui: &mut Ui) {
        match self.activity {
            ActivityBar::Agent => self.navigation(ui),
            ActivityBar::Explorer => self.explorer_panel(ui),
            ActivityBar::Search => self.search_panel(ui),
            ActivityBar::SourceControl => self.source_control_panel(ui),
        }
    }

    /// Explorer: the repository file tree.
    fn explorer_panel(&mut self, ui: &mut Ui) {
        let name = self.repository_name();
        self.panel_header(ui, "Explorer", Some(&name));
        let repository = self.repository.clone();
        if repository.as_os_str().is_empty() {
            ui.label(
                RichText::new("No folder opened")
                    .small()
                    .color(self.tokens.text_muted),
            );
            return;
        }
        let changed = self.changed_path_index();
        egui::ScrollArea::vertical()
            .id_salt("explorer_tree")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                self.render_tree_level(ui, &repository, &repository, 0, &changed);
            });
    }

    /// A panel header: the region's name, and optionally what it is scoped to.
    ///
    /// One line, at the top of every column, so the three columns read as one
    /// product rather than three widgets that happen to share a window.
    pub(crate) fn panel_header(&mut self, ui: &mut Ui, title: &str, subject: Option<&str>) {
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(title.to_ascii_uppercase())
                    .size(theme::TYPE_EYEBROW)
                    .strong()
                    .color(self.tokens.text_muted),
            );
            if let Some(subject) = subject {
                ui.label(
                    RichText::new(subject)
                        .size(theme::TYPE_EYEBROW)
                        .color(self.tokens.text_muted.gamma_multiply(0.8)),
                );
            }
        });
        ui.add_space(6.0);
    }

    /// Source Control: git changes.
    fn source_control_panel(&mut self, ui: &mut Ui) {
        let branch = self.workspace_state.branch.clone();
        self.panel_header(ui, "Source Control", branch.as_deref());
        // Same data as the aux panel and the status bar (FR-C9): the open
        // folder's uncommitted changes, so a bare "No changes" can never sit
        // next to a status bar that says "N uncommitted files".
        let changes = self.workspace_changes.clone();
        if !changes.available {
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new("Not checked yet")
                        .small()
                        .color(self.tokens.text_muted),
                );
                if ui.small_button("Retry").clicked() {
                    self.request_workspace_changes();
                }
            });
            return;
        }
        if changes.files_changed == 0 {
            ui.label(
                RichText::new("No changes")
                    .small()
                    .color(self.tokens.text_muted),
            );
            return;
        }
        ui.label(
            RichText::new(changes.summary())
                .size(theme::TYPE_EYEBROW)
                .color(self.tokens.text_muted),
        );
        ui.add_space(4.0);
        let entries = changes.entries.clone();
        let scope = changes.scope;
        egui::ScrollArea::vertical()
            .id_salt("source_control_list")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for entry in &entries {
                    self.changed_file_row(ui, entry, scope);
                }
            });
    }

    /// The right auxiliary panel: Agent session (with contextual tabs) or the
    /// change-review / source surface.
    fn aux_panel(&mut self, ui: &mut Ui, view: AuxView) {
        // There must be exactly one Agent surface in a frame. The normal
        // split path sets `agent_location` to `Aux` before choosing
        // `AuxView::Agent`; a stale/legacy toggle can otherwise leave the
        // centre Agent tab mounted as well, which renders the universal
        // composer twice and trips egui's duplicate widget-ID diagnostics.
        // Keep the panel useful and read-only in that inconsistent state.
        let view = effective_aux_view(view, self.agent_location);

        // The panel's own title strip, drawn in the chrome colour so the panel
        // reads as a docked surface rather than as content that floated loose.
        egui::Frame::new()
            .fill(self.tokens.background_secondary)
            .inner_margin(egui::Margin::symmetric(10, 0))
            .show(ui, |ui| {
                ui.set_height(theme::TAB_BAR_HEIGHT);
                ui.horizontal_centered(|ui| {
                    if view == AuxView::Agent {
                        crate::icons::brand_badge(ui, 18.0);
                    } else {
                        crate::icons::inline(
                            ui,
                            crate::icons::Glyph::Diff,
                            13.0,
                            self.tokens.text_secondary,
                        );
                    }
                    let title = match view {
                        AuxView::Agent => {
                            let title = self.session.title();
                            if title == "Untitled session" {
                                "Agent".to_owned()
                            } else {
                                truncate_title(title, 40)
                            }
                        }
                        // The panel is titled by what it actually shows, never
                        // by a hard-coded label: a source file is not "Changes".
                        AuxView::Source => {
                            if self.code_panel == CodePanel::Changes {
                                "Changes".to_owned()
                            } else if self.open_files.is_empty() {
                                // No file is open, so `code_column` shows the
                                // changes panel by default (FR-C3).
                                "Changes".to_owned()
                            } else {
                                "Source".to_owned()
                            }
                        }
                        AuxView::References => "References".to_owned(),
                        AuxView::Outline => "Outline".to_owned(),
                        AuxView::Checkpoints => "Checkpoints".to_owned(),
                    };
                    ui.label(RichText::new(title).color(self.tokens.text_primary));
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if self
                            .icon_action(ui, crate::icons::Glyph::Close, "Close panel")
                            .clicked()
                        {
                            self.aux_panel = None;
                        }
                    });
                });
            });
        ui.painter().hline(
            ui.max_rect().x_range(),
            ui.min_rect().bottom(),
            self.tokens.hairline(),
        );

        egui::Frame::new()
            .inner_margin(egui::Margin {
                left: 10,
                right: 10,
                top: 8,
                bottom: 8,
            })
            .show(ui, |ui| match view {
                AuxView::Agent => self.agent_surface(ui),
                AuxView::Source => self.code_column(ui),
                AuxView::References => self.references_panel(ui),
                AuxView::Outline => self.outline_panel(ui),
                AuxView::Checkpoints => self.checkpoints_panel(ui),
            });
    }

    /// The bottom panel: Terminal / Problems / Tests / Activity.
    fn bottom_panel(&mut self, ui: &mut Ui, kind: DockTab) {
        self.dock = Some(kind);
        self.dock_panel(ui);
        self.dock = None;
    }

    /// The editor center: a tab strip plus the active tab's content. The Agent
    /// session can occupy a tab, so one session renders here, in the aux panel,
    /// or as a focus view — never as two separate systems.
    fn editor_center(&mut self, ui: &mut Ui) {
        if self.title_visible {
            self.title_bar(ui);
        }
        self.editor_tabs(ui);
        self.breadcrumbs(ui);
        egui::Frame::new()
            .inner_margin(egui::Margin {
                left: 14,
                right: 14,
                top: 10,
                bottom: 10,
            })
            .show(ui, |ui| match self.agent_location {
                AgentLocation::EditorTab | AgentLocation::Focus => self.agent_surface(ui),
                AgentLocation::Aux => {
                    if self.active_file < self.open_files.len() {
                        self.code_editor(ui, self.active_file);
                    } else {
                        self.nothing_open(ui);
                    }
                }
            });
    }

    /// What the centre says when there is no file and no session on it.
    ///
    /// A sentence and the two things a user can do next — not a decorative
    /// card, and not an apology for an empty box (PRD §15).
    fn nothing_open(&mut self, ui: &mut Ui) {
        ui.vertical_centered(|ui| {
            ui.add_space((ui.available_height() * 0.30).max(24.0));
            crate::icons::inline(ui, crate::icons::Glyph::Files, 26.0, self.tokens.text_muted);
            ui.add_space(10.0);
            ui.label(RichText::new("No file open").color(self.tokens.text_secondary));
            ui.add_space(4.0);
            ui.label(
                RichText::new("⌘P to find a file, or ⌘1 for the agent")
                    .size(11.0)
                    .color(self.tokens.text_muted),
            );
        });
    }

    fn title_bar(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            crate::icons::brand_badge(ui, 16.0);
            ui.label(RichText::new(self.repository_name()).color(self.tokens.text_secondary));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.small_button("Hide title").clicked() {
                    self.title_visible = false;
                }
            });
        });
    }

    /// Global keyboard shortcuts (Cmd/Ctrl+1..4, B, P, Shift+P).
    fn handle_shortcuts(&mut self, ctx: &egui::Context) {
        use egui::{Key, Modifiers};
        // `consume_key` matches modifiers logically (extra modifiers such as a
        // stray Shift are ignored). For these shortcuts an exact match is the
        // point: Cmd+Shift+1 must not trigger the plain Cmd+1 rail shortcut.
        // So the modifier set is verified exactly first, then the key consumed.
        let pressed = |key: Key, shift: bool| {
            ctx.input_mut(|input| {
                let want_shift = shift;
                let mods_ok = input.modifiers.command
                    && !input.modifiers.alt
                    && !input.modifiers.ctrl
                    && input.modifiers.shift == want_shift;
                mods_ok && input.consume_key(Modifiers::COMMAND, key)
            })
        };
        for (i, item) in ActivityBar::ALL.iter().enumerate() {
            let key = match i {
                0 => Key::Num1,
                1 => Key::Num2,
                2 => Key::Num3,
                3 => Key::Num4,
                _ => continue,
            };
            if pressed(key, false) {
                self.activity = *item;
            }
        }
        if pressed(Key::B, false) {
            self.sidebar_visible = !self.sidebar_visible;
        }
        if pressed(Key::P, true) {
            self.command_palette = Some(String::new());
        } else if pressed(Key::P, false) {
            self.quick_open = Some(String::new());
        }
        if pressed(Key::J, false) {
            self.bottom_panel = if self.bottom_panel.is_some() {
                None
            } else {
                Some(DockTab::Terminal)
            };
        }
        if pressed(Key::S, false) && self.agent_location == AgentLocation::Aux {
            // ⌘S saves the active editor file when the source column owns the
            // centre. It is a no-op while the agent session holds the tab.
            self.save_and_format_active_file();
        }
        // ⌘⇧O opens the document outline, matching the "go to symbol" idiom.
        if pressed(Key::O, true)
            && let Some(path) = self.active_file_path()
        {
            self.aux_panel = Some(AuxView::Outline);
            self.request_symbols(&path);
        }

        // Language navigation. These are plain function keys, not ⌘ chords, so
        // they are checked outside the `pressed` helper above.
        let language_keys = ctx.input_mut(|input| {
            (
                input.consume_key(Modifiers::NONE, Key::F12),
                input.consume_key(Modifiers::SHIFT, Key::F12),
                input.consume_key(Modifiers::ALT.plus(Modifiers::SHIFT), Key::F),
            )
        });
        if let Some((path, position)) = self.caret_target() {
            let word = self.caret_word.clone();
            match language_keys {
                (true, _, _) => self.go_to_definition(&path, position),
                (_, true, _) => self.find_references(&path, position, &word),
                (_, _, true) => self.format_document(&path, false),
                _ => {}
            }
        }
    }

    /// The active editor file, when one owns the centre.
    fn active_file_path(&self) -> Option<PathBuf> {
        self.open_files
            .get(self.active_file)
            .map(|file| file.path.clone())
    }

    /// The document and position a language action should act on.
    ///
    /// `None` when no file is focused or the caret has not been placed, so a
    /// keypress does nothing rather than acting on a stale position from a
    /// file the user has since closed.
    fn caret_target(&self) -> Option<(PathBuf, crate::model::DocumentPosition)> {
        let path = self.active_file_path()?;
        Some((path, self.caret?))
    }

    /// ⌘S: format first when format-on-save is on, then write.
    ///
    /// The save is deferred until the formatting edits land, so the file on
    /// disk is the formatted text rather than the pre-format buffer followed
    /// by a second write. When no language server covers the file, this is an
    /// ordinary save.
    fn save_and_format_active_file(&mut self) {
        let Some(path) = self.active_file_path() else {
            return;
        };
        if self.format_on_save && self.language.supports(&path) {
            self.format_document(&path, true);
        } else {
            self.save_active_file();
        }
    }

    /// Command center overlay: ⌘P opens a file, ⌘⇧P runs a command.
    ///
    /// These were one surface over one file list, so "Command Palette" was a
    /// window titled after commands that only ever listed files. They are two
    /// surfaces now, because they answer two different questions.
    fn command_center(&mut self, ctx: &egui::Context) {
        let is_palette = self.command_palette.is_some();
        let mut query = if let Some(query) = &self.command_palette {
            query.clone()
        } else if let Some(query) = &self.quick_open {
            query.clone()
        } else {
            return;
        };
        let mut close = false;
        let mut open: Option<PathBuf> = None;
        let mut run: Option<PaletteCommand> = None;
        let commands = is_palette.then(|| self.palette_commands(&query));

        egui::Window::new(if is_palette {
            "Command Palette"
        } else {
            "Quick Open"
        })
        .anchor(egui::Align2::CENTER_TOP, egui::vec2(0.0, 40.0))
        .default_width(520.0)
        .resizable(false)
        .collapsible(false)
        .show(ctx, |ui| {
            let response = ui.add(
                egui::TextEdit::singleline(&mut query)
                    .desired_width(f32::INFINITY)
                    .hint_text(if is_palette {
                        "Type a command…"
                    } else {
                        "Type a file name…"
                    }),
            );
            response.request_focus();
            ui.separator();

            if let Some(commands) = &commands {
                if commands.is_empty() {
                    ui.label(
                        RichText::new("No command matches.")
                            .small()
                            .color(self.tokens.text_muted),
                    );
                }
                for command in commands.iter().take(12) {
                    let row = ui.selectable_label(false, &command.label);
                    if row.on_hover_text(&command.description).clicked() {
                        run = Some(command.clone());
                    }
                }
                if ui.input(|input| input.key_pressed(egui::Key::Enter))
                    && let Some(first) = commands.first()
                {
                    run = Some(first.clone());
                }
            } else {
                let results = self.fuzzy_files(&query);
                if results.is_empty() && !query.trim().is_empty() {
                    ui.label(
                        RichText::new("No file matches.")
                            .small()
                            .color(self.tokens.text_muted),
                    );
                }
                for (path, _) in results.iter().take(12) {
                    let shown = path.strip_prefix(&self.repository).unwrap_or(path);
                    if ui
                        .selectable_label(false, shown.display().to_string())
                        .clicked()
                    {
                        open = Some(path.clone());
                    }
                }
                if ui.input(|input| input.key_pressed(egui::Key::Enter))
                    && let Some(first) = results.first()
                {
                    open = Some(first.0.clone());
                }
            }
            if ui.input(|input| input.key_pressed(egui::Key::Escape)) {
                close = true;
            }
        });

        if is_palette {
            self.command_palette = Some(query);
        } else {
            self.quick_open = Some(query);
        }
        if let Some(path) = open {
            self.open_file(path);
            close = true;
        }
        // Dismiss before running: a command that opens another overlay (Go to
        // file) would otherwise have its own state cleared a line later.
        if close || run.is_some() {
            self.command_palette = None;
            self.quick_open = None;
        }
        if let Some(command) = run {
            self.run_palette_command(&command);
        }
    }

    /// The palette's entries: the window's own actions, then the daemon's
    /// session commands.
    ///
    /// Both are real. A window action does what it says immediately; a session
    /// command is placed in the composer for the agent, which is where the
    /// daemon's command contract is actually interpreted. Commands that need a
    /// session are not offered when none is selected, rather than being listed
    /// and then doing nothing.
    fn palette_commands(&self, query: &str) -> Vec<PaletteCommand> {
        let query = query.trim().to_lowercase();
        let mut out: Vec<PaletteCommand> = Vec::new();
        for (label, description, action) in [
            (
                "Go to file…",
                "Open a file by name",
                PaletteAction::QuickOpen,
            ),
            (
                "Go to symbol in file…",
                "List the symbols in the active file",
                PaletteAction::Outline,
            ),
            (
                "Format document",
                "Run the language server's formatter",
                PaletteAction::Format,
            ),
            (
                "Find in project…",
                "Search every file for text",
                PaletteAction::Search,
            ),
            (
                "Toggle terminal",
                "Show or hide the terminal panel",
                PaletteAction::Terminal,
            ),
            (
                "Show problems",
                "Diagnostics and validation failures",
                PaletteAction::Problems,
            ),
            (
                "Open settings",
                "Providers, models, MCP, skills",
                PaletteAction::Settings,
            ),
        ] {
            if label.to_lowercase().contains(&query) {
                out.push(PaletteCommand {
                    label: label.to_owned(),
                    description: description.to_owned(),
                    action,
                });
            }
        }
        if self.selected.is_some() {
            for (name, description, _) in &self.commands {
                if name.to_lowercase().contains(&query) {
                    out.push(PaletteCommand {
                        label: name.clone(),
                        description: description.clone(),
                        action: PaletteAction::Compose(name.clone()),
                    });
                }
            }
        }
        out
    }

    fn run_palette_command(&mut self, command: &PaletteCommand) {
        match &command.action {
            // The caller closes the palette after this returns, so setting
            // `quick_open` here hands the overlay straight to the file picker.
            PaletteAction::QuickOpen => self.quick_open = Some(String::new()),
            PaletteAction::Outline => {
                self.aux_panel = Some(AuxView::Outline);
                if let Some(path) = self.active_file_path() {
                    self.request_symbols(&path);
                }
            }
            PaletteAction::Format => {
                if let Some(path) = self.active_file_path() {
                    self.format_document(&path, false);
                }
            }
            PaletteAction::Search => self.activity = ActivityBar::Search,
            PaletteAction::Terminal => {
                self.bottom_panel = if self.bottom_panel == Some(DockTab::Terminal) {
                    None
                } else {
                    Some(DockTab::Terminal)
                };
            }
            PaletteAction::Problems => self.bottom_panel = Some(DockTab::Problems),
            PaletteAction::Settings => self.settings_open = true,
            PaletteAction::Compose(name) => {
                // Session commands are interpreted by the daemon from the
                // request text, so the palette puts the command where the user
                // can see it and add an argument before sending — rather than
                // firing it silently on their behalf.
                self.composer = format!("{name} ");
                self.composer_caret = self.composer.len();
                self.focus_composer = true;
            }
        }
    }

    /// Rank files by prefix/substring match against `query`.
    fn fuzzy_files(&self, query: &str) -> Vec<(PathBuf, i64)> {
        if query.trim().is_empty() {
            return Vec::new();
        }
        self.matching_paths(&query.to_ascii_lowercase(), false, 20)
            .into_iter()
            .map(|path| (path, 0))
            .collect()
    }

    /// Paths under the open folder matching a lowercase query.
    ///
    /// Matching is done against the repository-relative path, not the bare
    /// filename, so `app/mod` finds `src/app/mod.rs` while `mod` alone does
    /// not drown the list in every `mod.rs` in the tree. Results are ranked
    /// by where the match landed and how much path surrounds it, so the
    /// shortest, earliest match wins.
    pub(crate) fn matching_paths(
        &self,
        query: &str,
        directories: bool,
        limit: usize,
    ) -> Vec<PathBuf> {
        if self.repository.as_os_str().is_empty() {
            return Vec::new();
        }
        // A bounded walk. A repository with a pathological tree must not make
        // the composer stutter on every keystroke, so the crawl stops once it
        // has seen enough to rank a good answer.
        const MAX_VISITED: usize = 20_000;
        let mut scored: Vec<(i64, PathBuf)> = Vec::new();
        let mut queue = std::collections::VecDeque::from([self.repository.clone()]);
        let mut visited = 0_usize;
        while let Some(directory) = queue.pop_front() {
            let Ok(entries) = std::fs::read_dir(&directory) else {
                continue;
            };
            for entry in entries.flatten() {
                visited += 1;
                if visited > MAX_VISITED {
                    break;
                }
                let path = entry.path();
                let name = path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or_default();
                // The same exclusions the file tree uses, so completion and
                // the Explorer agree about what is in the project.
                if name.starts_with('.') || name == "target" || name == "node_modules" {
                    continue;
                }
                let is_directory = path.is_dir();
                if is_directory {
                    queue.push_back(path.clone());
                }
                if is_directory != directories {
                    continue;
                }
                let relative = path
                    .strip_prefix(&self.repository)
                    .unwrap_or(&path)
                    .to_string_lossy()
                    .to_ascii_lowercase();
                // An empty query lists what is nearest the root rather than
                // nothing: opening `@` with no text should show something.
                let position = if query.is_empty() {
                    Some(0)
                } else {
                    relative.find(query)
                };
                let Some(position) = position else {
                    continue;
                };
                scored.push((position as i64 + relative.len() as i64, path));
            }
        }
        scored.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
        scored.truncate(limit);
        scored.into_iter().map(|(_, path)| path).collect()
    }
}

/// Truncate a session title for a compact label, keeping the start and adding
/// an ellipsis. A `max` of 0 yields a bare ellipsis.
pub(crate) fn truncate_title(text: &str, max: usize) -> String {
    let trimmed = text.trim();
    if trimmed.chars().count() <= max {
        return trimmed.to_owned();
    }
    if max == 0 {
        return "…".to_owned();
    }
    let head: String = trimmed.chars().take(max.saturating_sub(1)).collect();
    format!("{head}…")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_folder_with_no_git_check_is_not_reported_clean() {
        let workspace = WorkspaceView::parse(&serde_json::json!({
            "is_git_repository": false,
            "git": { "status": "empty", "clean": true, "changed_file_count": 0 },
        }));
        assert!(!workspace.git_known);
        assert!(!workspace.is_git_repository);
    }

    #[test]
    fn a_configured_remote_is_not_an_authenticated_connection() {
        let workspace = WorkspaceView::parse(&serde_json::json!({
            "github": { "connected": false, "remote_configured": true, "authentication": "not_checked" },
        }));
        assert!(workspace.github_remote_configured);
    }

    #[test]
    fn activity_bar_order_matches_the_ide_hierarchy() {
        // Agent first (the signature surface), then Explorer, Search, and
        // Source Control (PRD §9-13).
        assert_eq!(
            ActivityBar::ALL,
            &[
                ActivityBar::Agent,
                ActivityBar::Explorer,
                ActivityBar::Search,
                ActivityBar::SourceControl,
            ]
        );
    }

    #[test]
    fn session_poll_waits_for_the_current_load_to_finish() {
        let interval = Duration::from_secs(1);
        assert!(should_poll_session(
            true,
            false,
            false,
            Duration::from_secs(2),
            interval,
        ));
        assert!(!should_poll_session(
            true,
            true,
            false,
            Duration::from_secs(2),
            interval,
        ));
        assert!(!should_poll_session(
            true,
            false,
            true,
            Duration::from_secs(2),
            interval,
        ));
    }

    #[test]
    fn stale_session_load_starts_cannot_replace_a_newer_generation() {
        assert!(session_load_start_is_current(None, 7));
        assert!(session_load_start_is_current(Some(7), 8));
        assert!(!session_load_start_is_current(Some(8), 8));
        assert!(!session_load_start_is_current(Some(8), 7));
    }

    #[test]
    fn background_session_refresh_preserves_a_populated_view() {
        assert!(preserve_session_during_refresh("session-1", false));
        assert!(!preserve_session_during_refresh("session-1", true));
        assert!(!preserve_session_during_refresh("", false));
    }

    #[test]
    fn an_aux_agent_view_never_duplicates_the_editor_composer() {
        assert_eq!(
            effective_aux_view(AuxView::Agent, AgentLocation::EditorTab),
            AuxView::Source
        );
        assert_eq!(
            effective_aux_view(AuxView::Agent, AgentLocation::Focus),
            AuxView::Source
        );
        assert_eq!(
            effective_aux_view(AuxView::Agent, AgentLocation::Aux),
            AuxView::Agent
        );
        assert_eq!(
            effective_aux_view(AuxView::Source, AgentLocation::EditorTab),
            AuxView::Source
        );
    }
}
