//! Declarative registry of every user-facing PurrCode capability.
//!
//! This module is the single source of truth for the command palette, the help
//! screen, contextual footer hints, command availability, disabled-state
//! explanations, and the acceptance-coverage report. Nothing may maintain a
//! second, manually synchronized list: `purrcode ui-actions list` and
//! `purrcode ui-actions coverage` render this registry, and the coverage gate in
//! `tests::coverage_gate` fails the build when an entry is incomplete.
//!
//! A capability is only considered discoverable when it is reachable from at
//! least one entry point (a command, a shortcut, or a focused decision surface)
//! and carries at least one acceptance scenario proving it can be executed.

use std::fmt;

// ── Identifiers ──────────────────────────────────────────────────

/// Stable identifier for a user-facing action. The string form appears in
/// coverage reports and acceptance documents, so it must not change casually.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct UiActionId(pub &'static str);

impl UiActionId {
    pub const fn as_str(self) -> &'static str {
        self.0
    }
}

impl fmt::Display for UiActionId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.0)
    }
}

/// Stable identifier for an acceptance scenario declared in [`SCENARIOS`].
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct AcceptanceScenarioId(pub &'static str);

impl AcceptanceScenarioId {
    pub const fn as_str(self) -> &'static str {
        self.0
    }
}

impl fmt::Display for AcceptanceScenarioId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.0)
    }
}

// ── Categories, risk, handlers ───────────────────────────────────

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum UiActionCategory {
    Task,
    Session,
    Provider,
    Model,
    Review,
    Approval,
    Recovery,
    Evidence,
    Skills,
    Settings,
    Help,
}

impl UiActionCategory {
    pub const ALL: &'static [Self] = &[
        Self::Task,
        Self::Session,
        Self::Provider,
        Self::Model,
        Self::Review,
        Self::Approval,
        Self::Recovery,
        Self::Evidence,
        Self::Skills,
        Self::Settings,
        Self::Help,
    ];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Task => "Task",
            Self::Session => "Session",
            Self::Provider => "Provider",
            Self::Model => "Model",
            Self::Review => "Review",
            Self::Approval => "Approval",
            Self::Recovery => "Recovery",
            Self::Evidence => "Evidence",
            Self::Skills => "Skills",
            Self::Settings => "Settings",
            Self::Help => "Help",
        }
    }
}

impl fmt::Display for UiActionCategory {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.label())
    }
}

/// How much authority an action carries. The palette and the focused decision
/// surfaces use this to decide whether a confirmation step is mandatory.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UiRiskClass {
    /// Reads state or changes only presentation.
    Safe,
    /// Changes durable local configuration or session lifecycle.
    Elevated,
    /// Discards work or authorizes a repository/network effect.
    Destructive,
    /// Grants authority to execute an exact action, or handles secrets.
    Security,
}

impl UiRiskClass {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Safe => "safe",
            Self::Elevated => "elevated",
            Self::Destructive => "destructive",
            Self::Security => "security",
        }
    }
}

/// Where an action is implemented. Every handler must resolve to a real entry
/// point; the coverage gate rejects a command that the dispatcher cannot serve.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UiActionHandler {
    /// Dispatched by `command_palette::CommandPalette::execute` under this verb.
    Command(&'static str),
    /// Opens a screen owned by the workbench shell.
    Screen(&'static str),
    /// Handled by a focused decision surface (approval, recovery, review).
    Decision(&'static str),
    /// Handled directly by the composer or shell key map.
    Shell(&'static str),
}

impl UiActionHandler {
    /// The dispatcher verb this handler needs, when it needs one.
    pub const fn command_verb(self) -> Option<&'static str> {
        match self {
            Self::Command(verb) => Some(verb),
            Self::Screen(_) | Self::Decision(_) | Self::Shell(_) => None,
        }
    }

    pub const fn kind(self) -> &'static str {
        match self {
            Self::Command(_) => "command",
            Self::Screen(_) => "screen",
            Self::Decision(_) => "decision",
            Self::Shell(_) => "shell",
        }
    }

    pub const fn target(self) -> &'static str {
        match self {
            Self::Command(target)
            | Self::Screen(target)
            | Self::Decision(target)
            | Self::Shell(target) => target,
        }
    }
}

// ── Shortcuts ────────────────────────────────────────────────────

/// Where a shortcut is live. A shortcut that only works inside a focused
/// decision surface must not be advertised on the conversation footer.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ShortcutContext {
    Workbench,
    Terminal,
    Approval,
    Review,
    Recovery,
    Palette,
    History,
    Streaming,
}

impl ShortcutContext {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Workbench => "workbench",
            Self::Terminal => "terminal",
            Self::Approval => "approval",
            Self::Review => "review",
            Self::Recovery => "recovery",
            Self::Palette => "palette",
            Self::History => "history",
            Self::Streaming => "streaming",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Shortcut {
    /// Human-readable key sequence, e.g. `Ctrl+G` or `A`.
    pub keys: &'static str,
    pub context: ShortcutContext,
    /// True when this shortcut is the primary, hint-worthy way to run the
    /// action. Common actions must have a primary shortcut *and* a visible
    /// hint so nothing depends on memorization.
    pub primary: bool,
}

impl Shortcut {
    pub const fn new(keys: &'static str, context: ShortcutContext) -> Self {
        Self {
            keys,
            context,
            primary: false,
        }
    }

    pub const fn primary(keys: &'static str, context: ShortcutContext) -> Self {
        Self {
            keys,
            context,
            primary: true,
        }
    }
}

// ── Availability ─────────────────────────────────────────────────

/// Everything an availability rule may inspect. Built once per frame from the
/// live application state so the palette, footer and help never disagree.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct UiContext {
    pub daemon_reachable: bool,
    pub provider_configured: bool,
    pub session_present: bool,
    pub session_active: bool,
    pub session_resumable: bool,
    pub session_read_only: bool,
    pub streaming: bool,
    pub pending_approval: bool,
    pub pending_model_pull: bool,
    pub active_model_pull: bool,
    pub repository_effects: bool,
    pub validation_attention: bool,
    pub recovery_required: bool,
    pub local_model_provider: bool,
    pub evidence_available: bool,
    pub composer_has_text: bool,
}

/// Result of evaluating an [`AvailabilityRule`]. Unavailable actions stay
/// visible in the palette with this explanation, so no surface presents a dead
/// end without a reason.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Availability {
    Available,
    Unavailable(&'static str),
}

impl Availability {
    pub const fn is_available(self) -> bool {
        matches!(self, Self::Available)
    }

    pub const fn reason(self) -> Option<&'static str> {
        match self {
            Self::Available => None,
            Self::Unavailable(reason) => Some(reason),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AvailabilityRule {
    Always,
    DaemonReachable,
    ProviderConfigured,
    SessionPresent,
    SessionActive,
    SessionResumable,
    SessionWritable,
    PendingApproval,
    PendingModelPull,
    ActiveModelPull,
    StreamingResponse,
    RepositoryEffects,
    ValidationAttention,
    RecoveryRequired,
    LocalModelProvider,
    EvidenceAvailable,
    ComposerHasText,
    /// Every nested rule must hold. The first failing rule supplies the reason.
    All(&'static [AvailabilityRule]),
}

impl AvailabilityRule {
    pub fn evaluate(&self, context: &UiContext) -> Availability {
        let unmet = |condition: bool, reason: &'static str| {
            if condition {
                Availability::Available
            } else {
                Availability::Unavailable(reason)
            }
        };
        match self {
            Self::Always => Availability::Available,
            Self::DaemonReachable => unmet(context.daemon_reachable, "the daemon is not reachable"),
            Self::ProviderConfigured => {
                unmet(context.provider_configured, "no provider is configured")
            }
            Self::SessionPresent => unmet(context.session_present, "no session is open"),
            Self::SessionActive => unmet(context.session_active, "the session is not active"),
            Self::SessionResumable => unmet(
                context.session_resumable,
                "the current session cannot be resumed",
            ),
            Self::SessionWritable => unmet(
                !context.session_read_only,
                "history is open read-only; start a new session first",
            ),
            Self::PendingApproval => unmet(context.pending_approval, "no action is pending"),
            Self::PendingModelPull => unmet(
                context.pending_model_pull,
                "no model pull is awaiting approval",
            ),
            Self::ActiveModelPull => unmet(context.active_model_pull, "no model pull is running"),
            Self::StreamingResponse => unmet(context.streaming, "nothing is generating"),
            Self::RepositoryEffects => unmet(
                context.repository_effects,
                "no repository effects were recorded",
            ),
            Self::ValidationAttention => unmet(
                context.validation_attention,
                "validation needs no attention",
            ),
            Self::RecoveryRequired => unmet(
                context.recovery_required,
                "the session does not require recovery",
            ),
            Self::LocalModelProvider => unmet(
                context.local_model_provider,
                "no local model provider is configured",
            ),
            Self::EvidenceAvailable => {
                unmet(context.evidence_available, "no evidence is recorded yet")
            }
            Self::ComposerHasText => unmet(context.composer_has_text, "the composer is empty"),
            Self::All(rules) => rules
                .iter()
                .map(|rule| rule.evaluate(context))
                .find(|availability| !availability.is_available())
                .unwrap_or(Availability::Available),
        }
    }

    /// Short label for the coverage report's Availability column.
    pub fn label(&self) -> String {
        match self {
            Self::Always => "always".into(),
            Self::DaemonReachable => "daemon".into(),
            Self::ProviderConfigured => "provider".into(),
            Self::SessionPresent => "session".into(),
            Self::SessionActive => "session-active".into(),
            Self::SessionResumable => "session-resumable".into(),
            Self::SessionWritable => "session-writable".into(),
            Self::PendingApproval => "pending-approval".into(),
            Self::PendingModelPull => "pending-pull".into(),
            Self::ActiveModelPull => "active-pull".into(),
            Self::StreamingResponse => "streaming".into(),
            Self::RepositoryEffects => "repo-effects".into(),
            Self::ValidationAttention => "validation-attention".into(),
            Self::RecoveryRequired => "recovery-required".into(),
            Self::LocalModelProvider => "local-provider".into(),
            Self::EvidenceAvailable => "evidence".into(),
            Self::ComposerHasText => "composer-text".into(),
            Self::All(rules) => rules.iter().map(Self::label).collect::<Vec<_>>().join("+"),
        }
    }
}

// ── Action definition ────────────────────────────────────────────

#[derive(Clone, Copy, Debug)]
pub struct UiActionDefinition {
    pub id: UiActionId,
    pub category: UiActionCategory,
    pub label: &'static str,
    pub description: &'static str,
    /// Commands that run this action. The first entry is canonical and is what
    /// the palette dispatches on Enter.
    pub commands: &'static [&'static str],
    pub shortcuts: &'static [Shortcut],
    pub availability: AvailabilityRule,
    pub risk: UiRiskClass,
    /// True when running this action can cause the agent to execute something.
    ///
    /// This is the axis the read-only-history invariant is stated on: opening
    /// history must never start execution, but it may still inspect evidence and
    /// clean up work that already happened.
    pub starts_execution: bool,
    pub handler: UiActionHandler,
    pub acceptance_scenarios: &'static [AcceptanceScenarioId],
}

impl UiActionDefinition {
    /// The command the palette dispatches. Actions with no command are reached
    /// through a shortcut or a focused decision surface instead.
    pub fn primary_command(&self) -> Option<&'static str> {
        self.commands.first().copied()
    }

    pub fn primary_shortcut(&self) -> Option<Shortcut> {
        self.shortcuts
            .iter()
            .find(|shortcut| shortcut.primary)
            .or_else(|| self.shortcuts.first())
            .copied()
    }

    pub fn availability(&self, context: &UiContext) -> Availability {
        self.availability.evaluate(context)
    }

    /// Surfaces from which a user can find this action. An executable action
    /// with no discovery surface fails the coverage gate.
    pub fn entry_points(&self) -> Vec<String> {
        let mut points = Vec::new();
        for command in self.commands {
            points.push((*command).to_owned());
        }
        for shortcut in self.shortcuts {
            points.push(format!("{} ({})", shortcut.keys, shortcut.context.label()));
        }
        if self.commands.is_empty() && self.shortcuts.is_empty() {
            points.push(format!("{}:{}", self.handler.kind(), self.handler.target()));
        }
        points
    }

    /// True when this action is reachable through the given dispatcher verb,
    /// either as its handler or through one of its command entry points.
    pub fn serves_verb(&self, verb: &str) -> bool {
        self.handler.command_verb() == Some(verb)
            || self
                .commands
                .iter()
                .filter_map(|command| command.trim_start_matches('/').split_whitespace().next())
                .any(|first| first == verb)
    }

    /// True when the action matches a palette query over label, description and
    /// commands.
    pub fn matches(&self, query: &str) -> bool {
        let query = query.trim().to_ascii_lowercase();
        if query.is_empty() {
            return true;
        }
        self.label.to_ascii_lowercase().contains(&query)
            || self.description.to_ascii_lowercase().contains(&query)
            || self.category.label().to_ascii_lowercase().contains(&query)
            || self
                .commands
                .iter()
                .any(|command| command.to_ascii_lowercase().contains(&query))
    }
}

// ── Acceptance scenarios ─────────────────────────────────────────

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ScenarioKind {
    /// Discoverable and executable on the happy path.
    Smoke,
    /// The action fails and the failure is understandable.
    Failure,
    /// The action is interrupted by the user.
    Cancellation,
    /// State survives a daemon or client restart.
    Restart,
    /// An uncertain or interrupted session can be recovered.
    Recovery,
}

impl ScenarioKind {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Smoke => "smoke",
            Self::Failure => "failure",
            Self::Cancellation => "cancellation",
            Self::Restart => "restart",
            Self::Recovery => "recovery",
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct AcceptanceScenario {
    pub id: AcceptanceScenarioId,
    pub summary: &'static str,
    pub kind: ScenarioKind,
    /// `tests/<file>.rs::<test name>` inside `crates/purrcode-tui-e2e`, when a
    /// PTY test proves this scenario.
    pub pty_test: Option<&'static str>,
    /// Case identifier inside `docs/ux/acceptance/*.md`, when the scenario is on
    /// the real-terminal checklist.
    pub real_terminal_case: Option<&'static str>,
    /// True when release gating requires this scenario to pass.
    pub critical: bool,
}

/// Lookup for a scenario declared in [`SCENARIOS`].
pub fn scenario(id: AcceptanceScenarioId) -> Option<&'static AcceptanceScenario> {
    SCENARIOS.iter().find(|scenario| scenario.id == id)
}

// ── The registry ─────────────────────────────────────────────────

const WORKBENCH: ShortcutContext = ShortcutContext::Workbench;

macro_rules! scenarios {
    ($($id:expr),* $(,)?) => { &[$(AcceptanceScenarioId($id)),*] };
}

/// Every user-facing capability PurrCode exposes.
pub const REGISTRY: &[UiActionDefinition] = &[
    // ── Task ───────────────────────────────────────────────────
    UiActionDefinition {
        id: UiActionId("task.submit"),
        category: UiActionCategory::Task,
        label: "Send task",
        description: "Send the composer contents to PurrCode",
        commands: &[],
        shortcuts: &[Shortcut::primary("Ctrl+G", WORKBENCH)],
        availability: AvailabilityRule::All(&[
            AvailabilityRule::SessionWritable,
            AvailabilityRule::ComposerHasText,
        ]),
        risk: UiRiskClass::Elevated,
        starts_execution: true,
        handler: UiActionHandler::Shell("composer.submit"),
        acceptance_scenarios: scenarios![
            "task.submit_first",
            "task.long_stream",
            "task.provider_failure",
        ],
    },
    UiActionDefinition {
        id: UiActionId("task.newline"),
        category: UiActionCategory::Task,
        label: "New line",
        description: "Add a line to the composer without sending",
        commands: &[],
        shortcuts: &[Shortcut::primary("Enter", WORKBENCH)],
        availability: AvailabilityRule::SessionWritable,
        risk: UiRiskClass::Safe,
        starts_execution: false,
        handler: UiActionHandler::Shell("composer.newline"),
        acceptance_scenarios: scenarios!["task.multiline_composer"],
    },
    UiActionDefinition {
        id: UiActionId("task.mode_ask"),
        category: UiActionCategory::Task,
        label: "Ask mode",
        description: "Answer questions about the repository without proposing changes",
        commands: &["/ask"],
        shortcuts: &[],
        availability: AvailabilityRule::Always,
        risk: UiRiskClass::Safe,
        starts_execution: false,
        handler: UiActionHandler::Command("ask"),
        acceptance_scenarios: scenarios!["task.mode_ask"],
    },
    UiActionDefinition {
        id: UiActionId("task.mode_plan"),
        category: UiActionCategory::Task,
        label: "Plan mode",
        description: "Produce a durable plan without modifying files",
        commands: &["/plan"],
        shortcuts: &[],
        availability: AvailabilityRule::Always,
        risk: UiRiskClass::Safe,
        starts_execution: false,
        handler: UiActionHandler::Command("plan"),
        acceptance_scenarios: scenarios!["task.mode_plan", "task.plan_revision"],
    },
    UiActionDefinition {
        id: UiActionId("task.mode_build"),
        category: UiActionCategory::Task,
        label: "Build mode",
        description: "Plan and propose repository changes for approval",
        commands: &["/build"],
        shortcuts: &[],
        availability: AvailabilityRule::Always,
        risk: UiRiskClass::Safe,
        starts_execution: false,
        handler: UiActionHandler::Command("build"),
        acceptance_scenarios: scenarios!["task.mode_build"],
    },
    UiActionDefinition {
        id: UiActionId("task.mode_review"),
        category: UiActionCategory::Task,
        label: "Review mode",
        description: "Review recorded work instead of producing new changes",
        commands: &["/review"],
        shortcuts: &[],
        availability: AvailabilityRule::Always,
        risk: UiRiskClass::Safe,
        starts_execution: false,
        handler: UiActionHandler::Command("review"),
        acceptance_scenarios: scenarios!["task.mode_review"],
    },
    UiActionDefinition {
        id: UiActionId("task.cancel_generation"),
        category: UiActionCategory::Task,
        label: "Cancel generation",
        description: "Stop the current response and keep the partial output",
        commands: &["/cancel"],
        shortcuts: &[Shortcut::primary("C", ShortcutContext::Streaming)],
        availability: AvailabilityRule::StreamingResponse,
        risk: UiRiskClass::Elevated,
        starts_execution: false,
        handler: UiActionHandler::Command("cancel"),
        acceptance_scenarios: scenarios!["task.cancel_streaming", "session.cancel_then_resume"],
    },
    UiActionDefinition {
        id: UiActionId("task.compact_context"),
        category: UiActionCategory::Task,
        label: "Compact context",
        description: "Summarize earlier turns to reclaim context budget",
        commands: &["/compact"],
        shortcuts: &[],
        availability: AvailabilityRule::All(&[
            AvailabilityRule::DaemonReachable,
            AvailabilityRule::SessionPresent,
        ]),
        risk: UiRiskClass::Elevated,
        starts_execution: true,
        handler: UiActionHandler::Command("compact"),
        acceptance_scenarios: scenarios!["task.compact_context"],
    },
    UiActionDefinition {
        id: UiActionId("task.web_research"),
        category: UiActionCategory::Task,
        label: "Fetch web page",
        description: "Propose a bounded web fetch that requires explicit approval",
        commands: &["/research", "/research-approve"],
        shortcuts: &[],
        availability: AvailabilityRule::All(&[
            AvailabilityRule::DaemonReachable,
            AvailabilityRule::SessionPresent,
        ]),
        risk: UiRiskClass::Security,
        starts_execution: true,
        handler: UiActionHandler::Command("research"),
        acceptance_scenarios: scenarios!["task.research_requires_approval", "task.research_denied",],
    },
    // ── Session ────────────────────────────────────────────────
    UiActionDefinition {
        id: UiActionId("session.new"),
        category: UiActionCategory::Session,
        label: "New session",
        description: "Keep history and start a new task",
        commands: &["/new"],
        shortcuts: &[Shortcut::primary("N", ShortcutContext::History)],
        availability: AvailabilityRule::Always,
        risk: UiRiskClass::Elevated,
        starts_execution: false,
        handler: UiActionHandler::Command("new"),
        acceptance_scenarios: scenarios!["session.new", "startup.session_choice"],
    },
    UiActionDefinition {
        id: UiActionId("session.list"),
        category: UiActionCategory::Session,
        label: "Open session",
        description: "List durable sessions for this repository",
        commands: &["/sessions", "/history"],
        shortcuts: &[Shortcut::primary("Ctrl+H", WORKBENCH)],
        availability: AvailabilityRule::DaemonReachable,
        risk: UiRiskClass::Safe,
        starts_execution: false,
        handler: UiActionHandler::Command("sessions"),
        acceptance_scenarios: scenarios!["session.history_is_read_only"],
    },
    UiActionDefinition {
        id: UiActionId("session.attach"),
        category: UiActionCategory::Session,
        label: "Attach to session",
        description: "Open a specific durable session without starting work",
        commands: &["/session"],
        shortcuts: &[],
        availability: AvailabilityRule::DaemonReachable,
        risk: UiRiskClass::Elevated,
        starts_execution: false,
        handler: UiActionHandler::Command("session"),
        acceptance_scenarios: scenarios!["session.history_is_read_only"],
    },
    UiActionDefinition {
        id: UiActionId("session.resume"),
        category: UiActionCategory::Session,
        label: "Resume session",
        description: "Continue the selected session from its durable boundary",
        commands: &["/resume"],
        shortcuts: &[Shortcut::primary("R", ShortcutContext::Recovery)],
        availability: AvailabilityRule::All(&[
            AvailabilityRule::DaemonReachable,
            AvailabilityRule::SessionResumable,
        ]),
        risk: UiRiskClass::Elevated,
        starts_execution: true,
        handler: UiActionHandler::Command("resume"),
        acceptance_scenarios: scenarios![
            "session.restart_then_resume",
            "recovery.uncertain_session"
        ],
    },
    UiActionDefinition {
        id: UiActionId("session.pause"),
        category: UiActionCategory::Session,
        label: "Pause session",
        description: "Stop at the next safe boundary and keep all evidence",
        commands: &["/pause"],
        shortcuts: &[],
        availability: AvailabilityRule::All(&[
            AvailabilityRule::DaemonReachable,
            AvailabilityRule::SessionActive,
        ]),
        risk: UiRiskClass::Elevated,
        starts_execution: false,
        handler: UiActionHandler::Command("pause"),
        acceptance_scenarios: scenarios!["session.pause"],
    },
    UiActionDefinition {
        id: UiActionId("session.privacy_toggle"),
        category: UiActionCategory::Session,
        label: "Toggle privacy mode",
        description: "Switch between local-only and mixed inference",
        commands: &["/privacy"],
        shortcuts: &[],
        availability: AvailabilityRule::Always,
        risk: UiRiskClass::Security,
        starts_execution: false,
        handler: UiActionHandler::Command("privacy"),
        acceptance_scenarios: scenarios![
            "session.privacy_visible",
            "session.privacy_policy_blocked",
            "startup.monochrome",
        ],
    },
    UiActionDefinition {
        id: UiActionId("session.quit"),
        category: UiActionCategory::Session,
        label: "Quit PurrCode",
        description: "Leave the workbench; durable state is preserved",
        commands: &["/quit"],
        shortcuts: &[Shortcut::primary("q", WORKBENCH)],
        availability: AvailabilityRule::Always,
        risk: UiRiskClass::Safe,
        starts_execution: false,
        handler: UiActionHandler::Command("quit"),
        acceptance_scenarios: scenarios![
            "startup.first_launch",
            "startup.daemon_unavailable",
            "session.draft_restoration",
        ],
    },
    // ── Provider ───────────────────────────────────────────────
    UiActionDefinition {
        id: UiActionId("provider.connect"),
        category: UiActionCategory::Provider,
        label: "Connect provider",
        description: "Discover a local provider or configure an endpoint",
        commands: &["/connect"],
        shortcuts: &[],
        availability: AvailabilityRule::DaemonReachable,
        risk: UiRiskClass::Security,
        starts_execution: false,
        handler: UiActionHandler::Command("connect"),
        acceptance_scenarios: scenarios![
            "provider.connect_local",
            "provider.connect_remote_reference",
            "provider.invalid_endpoint",
        ],
    },
    UiActionDefinition {
        id: UiActionId("provider.import"),
        category: UiActionCategory::Provider,
        label: "Import provider from script",
        description: "Parse a pasted SDK or config example without executing it",
        commands: &["/connect import"],
        shortcuts: &[],
        availability: AvailabilityRule::DaemonReachable,
        risk: UiRiskClass::Security,
        starts_execution: false,
        handler: UiActionHandler::Command("connect"),
        acceptance_scenarios: scenarios![
            "provider.import_configuration",
            "provider.secret_guard",
            "provider.import_unparseable",
        ],
    },
    UiActionDefinition {
        id: UiActionId("provider.list"),
        category: UiActionCategory::Provider,
        label: "List providers",
        description: "Show configured provider profiles and their privacy class",
        commands: &["/provider list", "/providers"],
        shortcuts: &[],
        availability: AvailabilityRule::DaemonReachable,
        risk: UiRiskClass::Safe,
        starts_execution: false,
        handler: UiActionHandler::Command("provider"),
        acceptance_scenarios: scenarios!["provider.list"],
    },
    UiActionDefinition {
        id: UiActionId("provider.test"),
        category: UiActionCategory::Provider,
        label: "Test provider",
        description: "Run a real connection test against a saved profile",
        commands: &["/provider test"],
        shortcuts: &[],
        availability: AvailabilityRule::DaemonReachable,
        risk: UiRiskClass::Safe,
        starts_execution: false,
        handler: UiActionHandler::Command("provider"),
        acceptance_scenarios: scenarios!["provider.test", "provider.unavailable"],
    },
    UiActionDefinition {
        id: UiActionId("provider.edit"),
        category: UiActionCategory::Provider,
        label: "Edit provider",
        description: "Reopen a saved profile in the provider form",
        commands: &["/provider edit"],
        shortcuts: &[],
        availability: AvailabilityRule::DaemonReachable,
        risk: UiRiskClass::Security,
        starts_execution: false,
        handler: UiActionHandler::Command("provider"),
        acceptance_scenarios: scenarios![
            "provider.edit",
            "provider.edit_requires_explicit_replace"
        ],
    },
    UiActionDefinition {
        id: UiActionId("provider.remove"),
        category: UiActionCategory::Provider,
        label: "Remove provider",
        description: "Delete a saved provider profile",
        commands: &["/provider remove"],
        shortcuts: &[],
        availability: AvailabilityRule::DaemonReachable,
        risk: UiRiskClass::Destructive,
        starts_execution: false,
        handler: UiActionHandler::Command("provider"),
        acceptance_scenarios: scenarios!["provider.remove", "provider.remove_in_use"],
    },
    // ── Model ──────────────────────────────────────────────────
    UiActionDefinition {
        id: UiActionId("model.select"),
        category: UiActionCategory::Model,
        label: "Switch model",
        description: "Choose from the configured and reachable models",
        commands: &["/models"],
        shortcuts: &[Shortcut::primary("Ctrl+M", WORKBENCH)],
        availability: AvailabilityRule::All(&[
            AvailabilityRule::DaemonReachable,
            AvailabilityRule::ProviderConfigured,
        ]),
        risk: UiRiskClass::Elevated,
        starts_execution: false,
        handler: UiActionHandler::Command("models"),
        acceptance_scenarios: scenarios!["model.switch", "model.unavailable"],
    },
    UiActionDefinition {
        id: UiActionId("model.assign_role"),
        category: UiActionCategory::Model,
        label: "Assign model role",
        description: "Bind a model to the planner, coder or judge role",
        commands: &["/role"],
        shortcuts: &[],
        availability: AvailabilityRule::All(&[
            AvailabilityRule::DaemonReachable,
            AvailabilityRule::ProviderConfigured,
        ]),
        risk: UiRiskClass::Elevated,
        starts_execution: false,
        handler: UiActionHandler::Command("role"),
        acceptance_scenarios: scenarios!["model.assign_role"],
    },
    UiActionDefinition {
        id: UiActionId("model.recommend"),
        category: UiActionCategory::Model,
        label: "Recommend local model",
        description: "Show observed memory and qualification evidence",
        commands: &["/model recommend"],
        shortcuts: &[],
        availability: AvailabilityRule::All(&[
            AvailabilityRule::DaemonReachable,
            AvailabilityRule::LocalModelProvider,
        ]),
        risk: UiRiskClass::Safe,
        starts_execution: false,
        handler: UiActionHandler::Command("model"),
        acceptance_scenarios: scenarios!["model.recommend", "model.insufficient_memory"],
    },
    UiActionDefinition {
        id: UiActionId("model.qualify"),
        category: UiActionCategory::Model,
        label: "Qualify local model",
        description: "Run a real provider-backed qualification suite",
        commands: &["/model qualify"],
        shortcuts: &[],
        availability: AvailabilityRule::All(&[
            AvailabilityRule::DaemonReachable,
            AvailabilityRule::LocalModelProvider,
        ]),
        risk: UiRiskClass::Elevated,
        starts_execution: false,
        handler: UiActionHandler::Command("model"),
        acceptance_scenarios: scenarios!["model.qualify"],
    },
    UiActionDefinition {
        id: UiActionId("model.loaded"),
        category: UiActionCategory::Model,
        label: "Inspect loaded models",
        description: "Show resident local models without loading one",
        commands: &["/model loaded"],
        shortcuts: &[],
        availability: AvailabilityRule::All(&[
            AvailabilityRule::DaemonReachable,
            AvailabilityRule::LocalModelProvider,
        ]),
        risk: UiRiskClass::Safe,
        starts_execution: false,
        handler: UiActionHandler::Command("model"),
        acceptance_scenarios: scenarios!["model.loaded"],
    },
    UiActionDefinition {
        id: UiActionId("model.unload"),
        category: UiActionCategory::Model,
        label: "Unload local models",
        description: "Release local model memory and verify the result",
        commands: &["/model unload-all", "/model unload"],
        shortcuts: &[],
        availability: AvailabilityRule::All(&[
            AvailabilityRule::DaemonReachable,
            AvailabilityRule::LocalModelProvider,
        ]),
        risk: UiRiskClass::Elevated,
        starts_execution: false,
        handler: UiActionHandler::Command("model"),
        acceptance_scenarios: scenarios!["model.unload"],
    },
    UiActionDefinition {
        id: UiActionId("model.pull_propose"),
        category: UiActionCategory::Model,
        label: "Pull local model",
        description: "Propose a model download that requires explicit approval",
        commands: &["/model pull"],
        shortcuts: &[],
        availability: AvailabilityRule::All(&[
            AvailabilityRule::DaemonReachable,
            AvailabilityRule::LocalModelProvider,
        ]),
        risk: UiRiskClass::Security,
        starts_execution: false,
        handler: UiActionHandler::Command("model"),
        acceptance_scenarios: scenarios!["model.pull_propose", "model.pull_reject"],
    },
    UiActionDefinition {
        id: UiActionId("model.pull_approve"),
        category: UiActionCategory::Model,
        label: "Approve model pull",
        description: "Authorize the exact proposed model download",
        commands: &["/model pull-approve"],
        shortcuts: &[Shortcut::primary("P", WORKBENCH)],
        availability: AvailabilityRule::PendingModelPull,
        risk: UiRiskClass::Security,
        starts_execution: true,
        handler: UiActionHandler::Command("model"),
        acceptance_scenarios: scenarios!["model.pull_approve", "model.pull_approve_refused"],
    },
    UiActionDefinition {
        id: UiActionId("model.pull_cancel"),
        category: UiActionCategory::Model,
        label: "Cancel model pull",
        description: "Stop a running download and wait for terminal evidence",
        commands: &["/model pull-cancel"],
        shortcuts: &[Shortcut::primary("C", WORKBENCH)],
        availability: AvailabilityRule::ActiveModelPull,
        risk: UiRiskClass::Elevated,
        starts_execution: false,
        handler: UiActionHandler::Command("model"),
        acceptance_scenarios: scenarios!["model.pull_cancel"],
    },
    // ── Review ─────────────────────────────────────────────────
    UiActionDefinition {
        id: UiActionId("review.open_diff"),
        category: UiActionCategory::Review,
        label: "Review diff",
        description: "Open the daemon-backed diff for this session",
        commands: &["/diff"],
        shortcuts: &[Shortcut::primary("Ctrl+D", WORKBENCH)],
        availability: AvailabilityRule::All(&[
            AvailabilityRule::DaemonReachable,
            AvailabilityRule::RepositoryEffects,
        ]),
        risk: UiRiskClass::Safe,
        starts_execution: false,
        handler: UiActionHandler::Command("diff"),
        acceptance_scenarios: scenarios![
            "review.open_diff",
            "review.multiple_files",
            "review.large_diff",
        ],
    },
    UiActionDefinition {
        id: UiActionId("review.next_file"),
        category: UiActionCategory::Review,
        label: "Next changed file",
        description: "Move to the next file in the review",
        commands: &[],
        shortcuts: &[Shortcut::primary("J", ShortcutContext::Review)],
        availability: AvailabilityRule::RepositoryEffects,
        risk: UiRiskClass::Safe,
        starts_execution: false,
        handler: UiActionHandler::Decision("review.next_file"),
        acceptance_scenarios: scenarios!["review.multiple_files"],
    },
    UiActionDefinition {
        id: UiActionId("review.previous_file"),
        category: UiActionCategory::Review,
        label: "Previous changed file",
        description: "Move to the previous file in the review",
        commands: &[],
        shortcuts: &[Shortcut::primary("K", ShortcutContext::Review)],
        availability: AvailabilityRule::RepositoryEffects,
        risk: UiRiskClass::Safe,
        starts_execution: false,
        handler: UiActionHandler::Decision("review.previous_file"),
        acceptance_scenarios: scenarios!["review.multiple_files"],
    },
    UiActionDefinition {
        id: UiActionId("review.next_hunk"),
        category: UiActionCategory::Review,
        label: "Next hunk",
        description: "Move to the next hunk in the selected file",
        commands: &[],
        shortcuts: &[Shortcut::primary("N", ShortcutContext::Review)],
        availability: AvailabilityRule::RepositoryEffects,
        risk: UiRiskClass::Safe,
        starts_execution: false,
        handler: UiActionHandler::Decision("review.next_hunk"),
        acceptance_scenarios: scenarios!["review.large_diff"],
    },
    UiActionDefinition {
        id: UiActionId("review.validation_summary"),
        category: UiActionCategory::Review,
        label: "Validation summary",
        description: "Show whether validation passed, failed, timed out or was unavailable",
        commands: &[],
        shortcuts: &[Shortcut::primary("V", ShortcutContext::Review)],
        availability: AvailabilityRule::RepositoryEffects,
        risk: UiRiskClass::Safe,
        starts_execution: false,
        handler: UiActionHandler::Decision("review.validation"),
        acceptance_scenarios: scenarios![
            "review.validation_failed",
            "review.validation_unavailable",
            "review.validation_timeout",
        ],
    },
    UiActionDefinition {
        id: UiActionId("review.rollback"),
        category: UiActionCategory::Review,
        label: "Roll back agent work",
        description: "Discard agent-owned changes inside the isolated worktree",
        commands: &["/rollback"],
        shortcuts: &[],
        availability: AvailabilityRule::All(&[
            AvailabilityRule::DaemonReachable,
            AvailabilityRule::RepositoryEffects,
        ]),
        risk: UiRiskClass::Destructive,
        starts_execution: false,
        handler: UiActionHandler::Command("rollback"),
        acceptance_scenarios: scenarios!["review.rollback", "review.rollback_unavailable"],
    },
    // ── Approval ───────────────────────────────────────────────
    UiActionDefinition {
        id: UiActionId("approval.approve"),
        category: UiActionCategory::Approval,
        label: "Approve action",
        description: "Authorize the exact pending action by its digest",
        commands: &["/approve"],
        shortcuts: &[Shortcut::primary("A", ShortcutContext::Approval)],
        availability: AvailabilityRule::All(&[
            AvailabilityRule::DaemonReachable,
            AvailabilityRule::PendingApproval,
        ]),
        risk: UiRiskClass::Security,
        starts_execution: true,
        handler: UiActionHandler::Command("approve"),
        acceptance_scenarios: scenarios![
            "approval.approve_exact_write",
            "approval.digest_mismatch",
            "approval.restart_preserves_boundary",
            "approval.bare_word_never_approves",
        ],
    },
    UiActionDefinition {
        id: UiActionId("approval.reject"),
        category: UiActionCategory::Approval,
        label: "Reject action",
        description: "Refuse the exact pending action without executing it",
        commands: &["/deny"],
        shortcuts: &[Shortcut::primary("R", ShortcutContext::Approval)],
        availability: AvailabilityRule::All(&[
            AvailabilityRule::DaemonReachable,
            AvailabilityRule::PendingApproval,
        ]),
        risk: UiRiskClass::Security,
        starts_execution: false,
        handler: UiActionHandler::Command("deny"),
        acceptance_scenarios: scenarios![
            "approval.reject_exact_write",
            "approval.reject_of_stale_action",
        ],
    },
    UiActionDefinition {
        id: UiActionId("approval.inspect"),
        category: UiActionCategory::Approval,
        label: "Inspect pending action",
        description: "Show the exact action, its scope, limits and affected paths",
        commands: &[],
        shortcuts: &[Shortcut::primary("D", ShortcutContext::Approval)],
        availability: AvailabilityRule::PendingApproval,
        risk: UiRiskClass::Safe,
        starts_execution: false,
        handler: UiActionHandler::Decision("approval.inspect"),
        acceptance_scenarios: scenarios!["approval.inspect_paths"],
    },
    UiActionDefinition {
        id: UiActionId("approval.instruct"),
        category: UiActionCategory::Approval,
        label: "Add instruction instead",
        description: "Leave the action pending and send guidance to PurrCode",
        commands: &[],
        shortcuts: &[Shortcut::primary("I", ShortcutContext::Approval)],
        availability: AvailabilityRule::PendingApproval,
        risk: UiRiskClass::Safe,
        starts_execution: false,
        handler: UiActionHandler::Decision("approval.instruct"),
        acceptance_scenarios: scenarios!["approval.bare_word_never_approves"],
    },
    UiActionDefinition {
        id: UiActionId("approval.leave_pending"),
        category: UiActionCategory::Approval,
        label: "Leave pending",
        description: "Return to the workbench without deciding",
        commands: &[],
        shortcuts: &[Shortcut::primary("Esc", ShortcutContext::Approval)],
        availability: AvailabilityRule::PendingApproval,
        risk: UiRiskClass::Safe,
        starts_execution: false,
        handler: UiActionHandler::Decision("approval.leave_pending"),
        acceptance_scenarios: scenarios!["approval.restart_preserves_boundary"],
    },
    // ── Recovery ───────────────────────────────────────────────
    UiActionDefinition {
        id: UiActionId("recovery.open"),
        category: UiActionCategory::Recovery,
        label: "Open recovery",
        description: "Show why the session is uncertain and what can be done",
        commands: &[],
        shortcuts: &[Shortcut::primary("Ctrl+R", WORKBENCH)],
        availability: AvailabilityRule::RecoveryRequired,
        risk: UiRiskClass::Safe,
        starts_execution: false,
        handler: UiActionHandler::Decision("recovery.open"),
        acceptance_scenarios: scenarios!["recovery.uncertain_session", "recovery.lease_conflict"],
    },
    UiActionDefinition {
        id: UiActionId("recovery.read_only_history"),
        category: UiActionCategory::Recovery,
        label: "Open read-only history",
        description: "Inspect a session without starting or resuming work",
        commands: &[],
        shortcuts: &[Shortcut::primary("O", ShortcutContext::Recovery)],
        availability: AvailabilityRule::SessionPresent,
        risk: UiRiskClass::Safe,
        starts_execution: false,
        handler: UiActionHandler::Decision("recovery.read_only"),
        acceptance_scenarios: scenarios!["session.history_is_read_only", "recovery.lease_conflict"],
    },
    UiActionDefinition {
        id: UiActionId("recovery.details"),
        category: UiActionCategory::Recovery,
        label: "Recovery details",
        description: "Show the durable state that blocks a safe resume",
        commands: &[],
        shortcuts: &[Shortcut::primary("D", ShortcutContext::Recovery)],
        availability: AvailabilityRule::RecoveryRequired,
        risk: UiRiskClass::Safe,
        starts_execution: false,
        handler: UiActionHandler::Decision("recovery.details"),
        acceptance_scenarios: scenarios!["recovery.lease_conflict"],
    },
    // ── Evidence ───────────────────────────────────────────────
    UiActionDefinition {
        id: UiActionId("evidence.trace_inspector"),
        category: UiActionCategory::Evidence,
        label: "Trace inspector",
        description: "Step through durable session events one at a time",
        commands: &[],
        shortcuts: &[Shortcut::primary("T", WORKBENCH)],
        availability: AvailabilityRule::EvidenceAvailable,
        risk: UiRiskClass::Safe,
        starts_execution: false,
        handler: UiActionHandler::Decision("evidence.trace"),
        acceptance_scenarios: scenarios!["evidence.trace_inspector", "evidence.unavailable"],
    },
    UiActionDefinition {
        id: UiActionId("evidence.inspect_activity"),
        category: UiActionCategory::Evidence,
        label: "Inspect activity",
        description: "Open the inspector for the selected activity entry",
        commands: &[],
        shortcuts: &[
            Shortcut::primary("E", WORKBENCH),
            Shortcut::new("Space", WORKBENCH),
        ],
        availability: AvailabilityRule::EvidenceAvailable,
        risk: UiRiskClass::Safe,
        starts_execution: false,
        handler: UiActionHandler::Decision("evidence.inspect_activity"),
        acceptance_scenarios: scenarios!["evidence.action_explanation"],
    },
    // ── Skills ─────────────────────────────────────────────────
    UiActionDefinition {
        id: UiActionId("skills.browse"),
        category: UiActionCategory::Skills,
        label: "Browse skills",
        description: "Inspect discovered and installed repository skills",
        commands: &["/skills"],
        shortcuts: &[],
        availability: AvailabilityRule::DaemonReachable,
        risk: UiRiskClass::Safe,
        starts_execution: false,
        handler: UiActionHandler::Command("skills"),
        acceptance_scenarios: scenarios!["skills.browse", "skills.unavailable"],
    },
    UiActionDefinition {
        id: UiActionId("skills.search"),
        category: UiActionCategory::Skills,
        label: "Search skills",
        description: "Find a capability by description before installing it",
        commands: &["/skills search", "/skill-search", "/skill-search-approve"],
        shortcuts: &[],
        availability: AvailabilityRule::DaemonReachable,
        risk: UiRiskClass::Safe,
        starts_execution: false,
        handler: UiActionHandler::Command("skill-search"),
        acceptance_scenarios: scenarios!["skills.search"],
    },
    UiActionDefinition {
        id: UiActionId("skills.download"),
        category: UiActionCategory::Skills,
        label: "Download skill",
        description: "Propose a pinned skill download that requires approval",
        commands: &["/skill-download", "/skill-download-approve"],
        shortcuts: &[],
        availability: AvailabilityRule::DaemonReachable,
        risk: UiRiskClass::Security,
        starts_execution: true,
        handler: UiActionHandler::Command("skill-download"),
        acceptance_scenarios: scenarios![
            "skills.download_requires_approval",
            "skills.download_signature_failure",
        ],
    },
    UiActionDefinition {
        id: UiActionId("skills.install"),
        category: UiActionCategory::Skills,
        label: "Install skill",
        description: "Propose a verified skill install that requires approval",
        commands: &["/skill-install", "/skill-install-approve"],
        shortcuts: &[Shortcut::primary("i", ShortcutContext::Palette)],
        availability: AvailabilityRule::DaemonReachable,
        risk: UiRiskClass::Security,
        starts_execution: true,
        handler: UiActionHandler::Command("skill-install"),
        acceptance_scenarios: scenarios![
            "skills.install_requires_approval",
            "skills.qualification_failure",
        ],
    },
    UiActionDefinition {
        id: UiActionId("skills.block_publisher"),
        category: UiActionCategory::Skills,
        label: "Block skill publisher",
        description: "Refuse all future skills from a publisher",
        commands: &["/skill-block"],
        shortcuts: &[],
        availability: AvailabilityRule::DaemonReachable,
        risk: UiRiskClass::Elevated,
        starts_execution: false,
        handler: UiActionHandler::Command("skill-block"),
        acceptance_scenarios: scenarios!["skills.block_publisher"],
    },
    UiActionDefinition {
        id: UiActionId("skills.mcp_tool"),
        category: UiActionCategory::Skills,
        label: "Find MCP capability",
        description: "Search for an MCP server that supplies a missing capability",
        commands: &["/mcp search", "/capability add"],
        shortcuts: &[],
        availability: AvailabilityRule::DaemonReachable,
        risk: UiRiskClass::Security,
        starts_execution: true,
        handler: UiActionHandler::Command("skill-search"),
        acceptance_scenarios: scenarios!["skills.mcp_approval", "skills.mcp_failure"],
    },
    // ── Modes ──────────────────────────────────────────────────
    UiActionDefinition {
        id: UiActionId("task.change_mode"),
        category: UiActionCategory::Task,
        label: "Change mode",
        description: "Switch between Ask, Plan, Build and Review",
        commands: &["/mode"],
        shortcuts: &[Shortcut::primary("Ctrl+K", WORKBENCH)],
        availability: AvailabilityRule::Always,
        risk: UiRiskClass::Safe,
        starts_execution: false,
        handler: UiActionHandler::Command("mode"),
        acceptance_scenarios: scenarios!["task.change_mode"],
    },
    UiActionDefinition {
        id: UiActionId("settings.permission_mode"),
        category: UiActionCategory::Settings,
        label: "Change permission mode",
        description: "Switch between Ask, Auto and Full Access",
        commands: &["/permission"],
        shortcuts: &[],
        // An authenticated human's authority decision is never gated on the
        // daemon being reachable: it must be expressible before the next run.
        availability: AvailabilityRule::Always,
        risk: UiRiskClass::Security,
        starts_execution: false,
        handler: UiActionHandler::Command("permission"),
        acceptance_scenarios: scenarios![
            "settings.permission_mode",
            "settings.permission_rejected",
        ],
    },
    UiActionDefinition {
        id: UiActionId("session.status"),
        category: UiActionCategory::Session,
        label: "Show technical status",
        description: "Repository path, commit state, full model id, session id, daemon and sandbox",
        commands: &["/status"],
        shortcuts: &[],
        availability: AvailabilityRule::Always,
        risk: UiRiskClass::Safe,
        starts_execution: false,
        handler: UiActionHandler::Command("status"),
        acceptance_scenarios: scenarios!["session.status"],
    },
    // ── Terminal ───────────────────────────────────────────────
    UiActionDefinition {
        id: UiActionId("terminal.open"),
        category: UiActionCategory::Review,
        label: "Open terminal",
        description: "Show the real PTY the agent runs commands in",
        commands: &["/terminal"],
        shortcuts: &[Shortcut::primary("Ctrl+T", WORKBENCH)],
        availability: AvailabilityRule::DaemonReachable,
        risk: UiRiskClass::Elevated,
        starts_execution: false,
        handler: UiActionHandler::Command("terminal"),
        acceptance_scenarios: scenarios!["terminal.open"],
    },
    UiActionDefinition {
        id: UiActionId("terminal.next"),
        category: UiActionCategory::Review,
        label: "Next terminal",
        description: "Move to the next terminal tab",
        commands: &[],
        shortcuts: &[Shortcut::primary("Tab", ShortcutContext::Terminal)],
        availability: AvailabilityRule::DaemonReachable,
        risk: UiRiskClass::Safe,
        starts_execution: false,
        handler: UiActionHandler::Decision("terminal.next"),
        acceptance_scenarios: scenarios!["terminal.open"],
    },
    UiActionDefinition {
        id: UiActionId("terminal.take_control"),
        category: UiActionCategory::Review,
        label: "Take terminal control",
        description: "Type into the terminal yourself; the process keeps running",
        commands: &["/terminal-take"],
        shortcuts: &[],
        availability: AvailabilityRule::DaemonReachable,
        risk: UiRiskClass::Elevated,
        starts_execution: false,
        handler: UiActionHandler::Command("terminal-take"),
        acceptance_scenarios: scenarios!["terminal.takeover"],
    },
    UiActionDefinition {
        id: UiActionId("terminal.return_control"),
        category: UiActionCategory::Review,
        label: "Return terminal control",
        description: "Hand the terminal back to the agent",
        commands: &["/terminal-return"],
        shortcuts: &[Shortcut::primary("Ctrl+W", ShortcutContext::Terminal)],
        availability: AvailabilityRule::DaemonReachable,
        risk: UiRiskClass::Elevated,
        starts_execution: false,
        handler: UiActionHandler::Command("terminal-return"),
        acceptance_scenarios: scenarios!["terminal.takeover"],
    },
    UiActionDefinition {
        id: UiActionId("session.open_studio"),
        category: UiActionCategory::Session,
        label: "Open Studio",
        description: "Open the graphical view of this same session",
        commands: &["/studio"],
        shortcuts: &[Shortcut::primary("Ctrl+Shift+S", WORKBENCH)],
        availability: AvailabilityRule::DaemonReachable,
        risk: UiRiskClass::Elevated,
        starts_execution: false,
        handler: UiActionHandler::Command("studio"),
        acceptance_scenarios: scenarios!["session.open_studio"],
    },
    // ── Settings ───────────────────────────────────────────────
    UiActionDefinition {
        id: UiActionId("settings.show"),
        category: UiActionCategory::Settings,
        label: "Settings",
        description: "Show provider, model and local-inference settings",
        commands: &["/settings"],
        shortcuts: &[],
        availability: AvailabilityRule::DaemonReachable,
        risk: UiRiskClass::Safe,
        starts_execution: false,
        handler: UiActionHandler::Command("settings"),
        acceptance_scenarios: scenarios!["settings.show"],
    },
    UiActionDefinition {
        id: UiActionId("settings.toggle_files"),
        category: UiActionCategory::Settings,
        label: "Toggle file panel",
        description: "Show or hide the repository file list",
        commands: &[],
        shortcuts: &[Shortcut::primary("Ctrl+B", WORKBENCH)],
        availability: AvailabilityRule::Always,
        risk: UiRiskClass::Safe,
        starts_execution: false,
        handler: UiActionHandler::Shell("workbench.toggle_files"),
        acceptance_scenarios: scenarios!["startup.narrow_terminal"],
    },
    UiActionDefinition {
        id: UiActionId("settings.toggle_inspector"),
        category: UiActionCategory::Settings,
        label: "Toggle inspector",
        description: "Show or hide the contextual inspector",
        commands: &[],
        // Not Ctrl+I: a terminal sends 0x09 for both Ctrl+I and Tab, so the two
        // are indistinguishable and Tab already moves focus.
        shortcuts: &[Shortcut::primary("Ctrl+O", WORKBENCH)],
        availability: AvailabilityRule::Always,
        risk: UiRiskClass::Safe,
        starts_execution: false,
        handler: UiActionHandler::Shell("workbench.toggle_inspector"),
        acceptance_scenarios: scenarios!["startup.narrow_terminal", "evidence.action_explanation"],
    },
    // ── Help ───────────────────────────────────────────────────
    UiActionDefinition {
        id: UiActionId("help.command_palette"),
        category: UiActionCategory::Help,
        label: "Command palette",
        description: "Search every available action by name or command",
        commands: &["/help"],
        shortcuts: &[
            Shortcut::primary("Ctrl+P", WORKBENCH),
            Shortcut::new("?", WORKBENCH),
        ],
        availability: AvailabilityRule::Always,
        risk: UiRiskClass::Safe,
        starts_execution: false,
        handler: UiActionHandler::Command("help"),
        acceptance_scenarios: scenarios!["help.command_palette", "help.unavailable_explained"],
    },
];

/// Every acceptance scenario referenced by [`REGISTRY`].
///
/// `pty_test` names a test in `crates/purrcode-tui-e2e/tests`; the e2e crate has
/// a gate that fails when a referenced test name is absent from its sources, so
/// this table cannot claim coverage that does not exist.
pub const SCENARIOS: &[AcceptanceScenario] = &[
    AcceptanceScenario {
        id: AcceptanceScenarioId("session.status"),
        summary: "Detail the header hides stays reachable on demand",
        kind: ScenarioKind::Smoke,
        pty_test: Some("tests/modes.rs::status_shows_what_the_header_deliberately_omits"),
        real_terminal_case: None,
        critical: false,
    },
    AcceptanceScenario {
        id: AcceptanceScenarioId("task.change_mode"),
        summary: "The task mode changes and the header follows",
        kind: ScenarioKind::Smoke,
        pty_test: Some("tests/modes.rs::task_mode_changes_and_the_header_follows"),
        real_terminal_case: None,
        critical: true,
    },
    AcceptanceScenario {
        id: AcceptanceScenarioId("settings.permission_rejected"),
        summary: "An unrecognised permission value is refused, not silently ignored",
        kind: ScenarioKind::Failure,
        pty_test: Some("tests/modes.rs::an_unknown_permission_mode_is_refused"),
        real_terminal_case: None,
        critical: true,
    },
    AcceptanceScenario {
        id: AcceptanceScenarioId("settings.permission_mode"),
        summary: "Ask, Auto and Full Access are selectable and shown",
        kind: ScenarioKind::Smoke,
        pty_test: Some("tests/modes.rs::permission_mode_is_selectable_and_visible"),
        real_terminal_case: None,
        critical: true,
    },
    AcceptanceScenario {
        id: AcceptanceScenarioId("terminal.open"),
        summary: "The terminal surface shows real PTY output with tabs",
        kind: ScenarioKind::Smoke,
        pty_test: Some("tests/terminal.rs::terminal_shows_real_pty_output"),
        real_terminal_case: None,
        critical: true,
    },
    AcceptanceScenario {
        id: AcceptanceScenarioId("terminal.takeover"),
        summary: "A human takes terminal control and hands it back",
        kind: ScenarioKind::Smoke,
        pty_test: Some("tests/terminal.rs::terminal_control_transfers_to_the_human_and_back"),
        real_terminal_case: None,
        critical: false,
    },
    AcceptanceScenario {
        id: AcceptanceScenarioId("session.open_studio"),
        summary: "Studio opens on the session the workbench already has",
        kind: ScenarioKind::Smoke,
        pty_test: Some("tests/terminal.rs::studio_opens_on_the_active_session"),
        real_terminal_case: None,
        critical: false,
    },
    // Startup and repository
    AcceptanceScenario {
        id: AcceptanceScenarioId("startup.first_launch"),
        summary: "First launch shows the workbench and an actionable next step",
        kind: ScenarioKind::Smoke,
        pty_test: Some("tests/startup.rs::first_launch_shows_workbench_and_next_action"),
        real_terminal_case: Some("RT-01"),
        critical: true,
    },
    AcceptanceScenario {
        id: AcceptanceScenarioId("startup.daemon_unavailable"),
        summary: "An unreachable daemon is reported with a recovery action",
        kind: ScenarioKind::Failure,
        pty_test: Some("tests/startup.rs::daemon_unavailable_reports_recovery_action"),
        real_terminal_case: None,
        critical: true,
    },
    AcceptanceScenario {
        id: AcceptanceScenarioId("startup.narrow_terminal"),
        summary: "A 60-column terminal keeps one focused surface usable",
        kind: ScenarioKind::Smoke,
        pty_test: Some("tests/accessibility.rs::narrow_terminal_keeps_one_focused_surface"),
        real_terminal_case: Some("RT-16"),
        critical: true,
    },
    AcceptanceScenario {
        id: AcceptanceScenarioId("startup.session_choice"),
        summary: "A recovered session requires an explicit resume-or-new choice",
        kind: ScenarioKind::Restart,
        pty_test: Some("tests/startup.rs::recovered_session_requires_explicit_choice"),
        real_terminal_case: Some("RT-11"),
        critical: true,
    },
    AcceptanceScenario {
        id: AcceptanceScenarioId("startup.monochrome"),
        summary: "NO_COLOR and monochrome keep every status distinguishable",
        kind: ScenarioKind::Smoke,
        pty_test: Some("tests/accessibility.rs::monochrome_keeps_status_distinguishable"),
        real_terminal_case: Some("RT-18"),
        critical: true,
    },
    // Provider
    AcceptanceScenario {
        id: AcceptanceScenarioId("provider.connect_local"),
        summary: "A local provider is discovered and saved after a real test",
        kind: ScenarioKind::Smoke,
        pty_test: Some("tests/provider.rs::local_provider_discovery_saves_after_real_test"),
        real_terminal_case: Some("RT-02"),
        critical: true,
    },
    AcceptanceScenario {
        id: AcceptanceScenarioId("provider.connect_remote_reference"),
        summary: "A remote provider is saved using a credential reference only",
        kind: ScenarioKind::Smoke,
        pty_test: Some("tests/provider.rs::remote_provider_saves_credential_reference_only"),
        real_terminal_case: Some("RT-03"),
        critical: true,
    },
    AcceptanceScenario {
        id: AcceptanceScenarioId("provider.import_configuration"),
        summary: "A pasted configuration is parsed without execution",
        kind: ScenarioKind::Smoke,
        pty_test: Some("tests/provider.rs::imported_configuration_is_parsed_not_executed"),
        real_terminal_case: None,
        critical: false,
    },
    AcceptanceScenario {
        id: AcceptanceScenarioId("provider.secret_guard"),
        summary: "Pasted secrets are redacted before entering history",
        kind: ScenarioKind::Smoke,
        pty_test: Some("tests/provider.rs::pasted_secret_is_redacted_before_history"),
        real_terminal_case: Some("RT-15"),
        critical: true,
    },
    AcceptanceScenario {
        id: AcceptanceScenarioId("provider.invalid_endpoint"),
        summary: "An invalid endpoint fails visibly and stays editable",
        kind: ScenarioKind::Failure,
        pty_test: Some("tests/provider.rs::invalid_endpoint_fails_visibly"),
        real_terminal_case: None,
        critical: true,
    },
    AcceptanceScenario {
        id: AcceptanceScenarioId("provider.unavailable"),
        summary: "An unreachable provider is reported without claiming success",
        kind: ScenarioKind::Failure,
        pty_test: Some("tests/provider.rs::unavailable_provider_never_reports_success"),
        real_terminal_case: None,
        critical: true,
    },
    AcceptanceScenario {
        id: AcceptanceScenarioId("provider.list"),
        summary: "Configured providers list with their privacy class",
        kind: ScenarioKind::Smoke,
        pty_test: Some("tests/provider.rs::provider_list_shows_privacy_class"),
        real_terminal_case: None,
        critical: false,
    },
    AcceptanceScenario {
        id: AcceptanceScenarioId("provider.test"),
        summary: "A saved profile runs a real connection test",
        kind: ScenarioKind::Smoke,
        pty_test: Some("tests/provider.rs::saved_profile_runs_real_connection_test"),
        real_terminal_case: None,
        critical: false,
    },
    AcceptanceScenario {
        id: AcceptanceScenarioId("provider.edit"),
        summary: "A saved profile reopens in the provider form",
        kind: ScenarioKind::Smoke,
        pty_test: Some("tests/provider.rs::saved_profile_reopens_in_form"),
        real_terminal_case: None,
        critical: false,
    },
    AcceptanceScenario {
        id: AcceptanceScenarioId("provider.remove"),
        summary: "A provider profile is removed and disappears from the list",
        kind: ScenarioKind::Smoke,
        pty_test: Some("tests/provider.rs::removed_profile_disappears_from_list"),
        real_terminal_case: None,
        critical: false,
    },
    // Model
    AcceptanceScenario {
        id: AcceptanceScenarioId("model.switch"),
        summary: "A configured model is selected and shown in the header",
        kind: ScenarioKind::Smoke,
        pty_test: Some("tests/model.rs::model_switch_updates_header"),
        real_terminal_case: Some("RT-13"),
        critical: true,
    },
    AcceptanceScenario {
        id: AcceptanceScenarioId("model.unavailable"),
        summary: "An unavailable model is refused with an explanation",
        kind: ScenarioKind::Failure,
        pty_test: Some("tests/model.rs::unavailable_model_is_refused_with_reason"),
        real_terminal_case: None,
        critical: true,
    },
    AcceptanceScenario {
        id: AcceptanceScenarioId("model.assign_role"),
        summary: "A model is bound to a role and the binding is confirmed",
        kind: ScenarioKind::Smoke,
        pty_test: Some("tests/model.rs::role_assignment_is_confirmed"),
        real_terminal_case: None,
        critical: false,
    },
    AcceptanceScenario {
        id: AcceptanceScenarioId("model.recommend"),
        summary: "Recommendations show observed resource evidence",
        kind: ScenarioKind::Smoke,
        pty_test: Some("tests/model.rs::recommendation_shows_observed_evidence"),
        real_terminal_case: None,
        critical: false,
    },
    AcceptanceScenario {
        id: AcceptanceScenarioId("model.insufficient_memory"),
        summary: "An oversized local model warns before a task starts",
        kind: ScenarioKind::Failure,
        pty_test: Some("tests/model.rs::insufficient_memory_warns_before_task"),
        real_terminal_case: None,
        critical: true,
    },
    AcceptanceScenario {
        id: AcceptanceScenarioId("model.qualify"),
        summary: "Qualification reports a real provider-backed result",
        kind: ScenarioKind::Smoke,
        pty_test: Some("tests/model.rs::qualification_reports_real_result"),
        real_terminal_case: None,
        critical: false,
    },
    AcceptanceScenario {
        id: AcceptanceScenarioId("model.loaded"),
        summary: "Loaded local models are listed without loading one",
        kind: ScenarioKind::Smoke,
        pty_test: Some("tests/model.rs::loaded_models_listed_without_loading"),
        real_terminal_case: None,
        critical: false,
    },
    AcceptanceScenario {
        id: AcceptanceScenarioId("model.unload"),
        summary: "Unloading local models is verified by rediscovery",
        kind: ScenarioKind::Smoke,
        pty_test: Some("tests/model.rs::unload_is_verified_by_rediscovery"),
        real_terminal_case: None,
        critical: false,
    },
    AcceptanceScenario {
        id: AcceptanceScenarioId("model.pull_propose"),
        summary: "A pull proposal shows its exact action identity and digest",
        kind: ScenarioKind::Smoke,
        pty_test: Some("tests/model.rs::pull_proposal_shows_exact_action_identity"),
        real_terminal_case: None,
        critical: true,
    },
    AcceptanceScenario {
        id: AcceptanceScenarioId("model.pull_approve"),
        summary: "Approving a pull calls the daemon approval endpoint",
        kind: ScenarioKind::Smoke,
        pty_test: Some("tests/model.rs::pull_approval_calls_daemon_endpoint"),
        real_terminal_case: None,
        critical: true,
    },
    AcceptanceScenario {
        id: AcceptanceScenarioId("model.pull_reject"),
        summary: "A pull left unapproved never downloads",
        kind: ScenarioKind::Failure,
        pty_test: Some("tests/model.rs::unapproved_pull_never_downloads"),
        real_terminal_case: None,
        critical: true,
    },
    AcceptanceScenario {
        id: AcceptanceScenarioId("model.pull_cancel"),
        summary: "A running pull is cancelled and reports terminal evidence",
        kind: ScenarioKind::Cancellation,
        pty_test: Some("tests/model.rs::running_pull_cancels_with_terminal_evidence"),
        real_terminal_case: None,
        critical: true,
    },
    // Task and conversation
    AcceptanceScenario {
        id: AcceptanceScenarioId("task.submit_first"),
        summary: "A first task is accepted and its progress is understandable",
        kind: ScenarioKind::Smoke,
        pty_test: Some("tests/conversation.rs::first_task_progress_is_understandable"),
        real_terminal_case: Some("RT-04"),
        critical: true,
    },
    AcceptanceScenario {
        id: AcceptanceScenarioId("task.long_stream"),
        summary: "A long streaming answer stays readable and scrollable",
        kind: ScenarioKind::Smoke,
        pty_test: Some("tests/conversation.rs::long_streaming_answer_stays_readable"),
        real_terminal_case: Some("RT-05"),
        critical: true,
    },
    AcceptanceScenario {
        id: AcceptanceScenarioId("task.multiline_composer"),
        summary: "Multiline input and bracketed paste do not send early",
        kind: ScenarioKind::Smoke,
        pty_test: Some("tests/conversation.rs::multiline_and_paste_do_not_send_early"),
        real_terminal_case: None,
        critical: true,
    },
    AcceptanceScenario {
        id: AcceptanceScenarioId("task.mode_ask"),
        summary: "Ask mode answers without proposing repository changes",
        kind: ScenarioKind::Smoke,
        pty_test: Some("tests/conversation.rs::ask_mode_proposes_no_changes"),
        real_terminal_case: None,
        critical: false,
    },
    AcceptanceScenario {
        id: AcceptanceScenarioId("task.mode_plan"),
        summary: "Plan mode produces a plan without modifying files",
        kind: ScenarioKind::Smoke,
        pty_test: Some("tests/conversation.rs::plan_mode_modifies_nothing"),
        real_terminal_case: None,
        critical: false,
    },
    AcceptanceScenario {
        id: AcceptanceScenarioId("task.plan_revision"),
        summary: "A plan under review is revised by replying to it",
        kind: ScenarioKind::Smoke,
        pty_test: Some("tests/conversation.rs::a_plan_under_review_is_revised_by_replying_to_it"),
        real_terminal_case: None,
        critical: false,
    },
    AcceptanceScenario {
        id: AcceptanceScenarioId("task.mode_build"),
        summary: "Build mode proposes changes that require approval",
        kind: ScenarioKind::Smoke,
        pty_test: Some("tests/conversation.rs::build_mode_requires_approval"),
        real_terminal_case: None,
        critical: false,
    },
    AcceptanceScenario {
        id: AcceptanceScenarioId("task.mode_review"),
        summary: "Review mode reports recorded work instead of new changes",
        kind: ScenarioKind::Smoke,
        pty_test: Some("tests/conversation.rs::review_mode_reports_recorded_work"),
        real_terminal_case: None,
        critical: false,
    },
    AcceptanceScenario {
        id: AcceptanceScenarioId("task.cancel_streaming"),
        summary: "Cancelling mid-stream preserves partial output",
        kind: ScenarioKind::Cancellation,
        pty_test: Some("tests/conversation.rs::cancel_midstream_preserves_partial_output"),
        real_terminal_case: Some("RT-10"),
        critical: true,
    },
    AcceptanceScenario {
        id: AcceptanceScenarioId("task.provider_failure"),
        summary: "A provider failure is explained with a retry path",
        kind: ScenarioKind::Failure,
        pty_test: Some("tests/conversation.rs::provider_failure_explains_retry_path"),
        real_terminal_case: None,
        critical: true,
    },
    AcceptanceScenario {
        id: AcceptanceScenarioId("task.compact_context"),
        summary: "Compaction reports what was summarized",
        kind: ScenarioKind::Smoke,
        pty_test: Some("tests/conversation.rs::compaction_reports_what_was_summarized"),
        real_terminal_case: None,
        critical: false,
    },
    AcceptanceScenario {
        id: AcceptanceScenarioId("task.research_requires_approval"),
        summary: "A web fetch is never performed without approval",
        kind: ScenarioKind::Smoke,
        pty_test: Some("tests/conversation.rs::web_fetch_requires_approval"),
        real_terminal_case: None,
        critical: true,
    },
    AcceptanceScenario {
        id: AcceptanceScenarioId("task.research_denied"),
        summary: "A rejected web fetch never reaches the network",
        kind: ScenarioKind::Failure,
        pty_test: Some("tests/conversation.rs::rejected_web_fetch_never_reaches_the_network"),
        real_terminal_case: None,
        critical: true,
    },
    // Approval
    AcceptanceScenario {
        id: AcceptanceScenarioId("approval.approve_exact_write"),
        summary: "Pressing A approves exactly the displayed action",
        kind: ScenarioKind::Smoke,
        pty_test: Some("tests/approval.rs::approve_key_authorizes_the_displayed_action"),
        real_terminal_case: Some("RT-06"),
        critical: true,
    },
    AcceptanceScenario {
        id: AcceptanceScenarioId("approval.reject_exact_write"),
        summary: "Rejecting an action prevents execution",
        kind: ScenarioKind::Smoke,
        pty_test: Some("tests/approval.rs::rejection_prevents_execution"),
        real_terminal_case: Some("RT-07"),
        critical: true,
    },
    AcceptanceScenario {
        id: AcceptanceScenarioId("approval.inspect_paths"),
        summary: "The approval surface shows every affected path and limit",
        kind: ScenarioKind::Smoke,
        pty_test: Some("tests/approval.rs::approval_shows_paths_scope_and_limits"),
        real_terminal_case: Some("RT-06"),
        critical: true,
    },
    AcceptanceScenario {
        id: AcceptanceScenarioId("approval.digest_mismatch"),
        summary: "A digest mismatch fails visibly and never executes",
        kind: ScenarioKind::Failure,
        pty_test: Some("tests/approval.rs::digest_mismatch_fails_visibly"),
        real_terminal_case: None,
        critical: true,
    },
    AcceptanceScenario {
        id: AcceptanceScenarioId("approval.restart_preserves_boundary"),
        summary: "Restarting while awaiting approval preserves the boundary",
        kind: ScenarioKind::Restart,
        pty_test: Some("tests/approval.rs::restart_preserves_pending_boundary"),
        real_terminal_case: Some("RT-11"),
        critical: true,
    },
    AcceptanceScenario {
        id: AcceptanceScenarioId("approval.bare_word_never_approves"),
        summary: "Natural-language text never silently approves",
        kind: ScenarioKind::Failure,
        pty_test: Some("tests/approval.rs::natural_language_never_approves"),
        real_terminal_case: None,
        critical: true,
    },
    // Review and validation
    AcceptanceScenario {
        id: AcceptanceScenarioId("review.open_diff"),
        summary: "The review screen shows daemon-backed changed files",
        kind: ScenarioKind::Smoke,
        pty_test: Some("tests/review.rs::review_shows_daemon_backed_changed_files"),
        real_terminal_case: Some("RT-08"),
        critical: true,
    },
    AcceptanceScenario {
        id: AcceptanceScenarioId("review.multiple_files"),
        summary: "File and hunk navigation works across multiple files",
        kind: ScenarioKind::Smoke,
        pty_test: Some("tests/review.rs::file_and_hunk_navigation_works"),
        real_terminal_case: Some("RT-08"),
        critical: true,
    },
    AcceptanceScenario {
        id: AcceptanceScenarioId("review.large_diff"),
        summary: "A large diff loads incrementally and stays responsive",
        kind: ScenarioKind::Smoke,
        pty_test: Some("tests/review.rs::large_diff_loads_incrementally"),
        real_terminal_case: None,
        critical: true,
    },
    AcceptanceScenario {
        id: AcceptanceScenarioId("review.validation_failed"),
        summary: "Failed validation is never presented as success",
        kind: ScenarioKind::Failure,
        pty_test: Some("tests/review.rs::failed_validation_is_not_success"),
        real_terminal_case: Some("RT-09"),
        critical: true,
    },
    AcceptanceScenario {
        id: AcceptanceScenarioId("review.validation_unavailable"),
        summary: "Unavailable validation stays visible as unavailable",
        kind: ScenarioKind::Failure,
        pty_test: Some("tests/review.rs::unavailable_validation_stays_visible"),
        real_terminal_case: None,
        critical: true,
    },
    AcceptanceScenario {
        id: AcceptanceScenarioId("review.validation_timeout"),
        summary: "A validation timeout is distinguished from a failure",
        kind: ScenarioKind::Failure,
        pty_test: Some("tests/review.rs::validation_timeout_is_distinct_from_failure"),
        real_terminal_case: None,
        critical: true,
    },
    AcceptanceScenario {
        id: AcceptanceScenarioId("review.rollback"),
        summary: "Rollback discards agent-owned work and says so",
        kind: ScenarioKind::Recovery,
        pty_test: Some("tests/review.rs::rollback_discards_agent_owned_work"),
        real_terminal_case: None,
        critical: true,
    },
    // Session lifecycle
    AcceptanceScenario {
        id: AcceptanceScenarioId("session.new"),
        summary: "A new session starts without destroying history",
        kind: ScenarioKind::Smoke,
        pty_test: Some("tests/recovery.rs::new_session_preserves_history"),
        real_terminal_case: None,
        critical: true,
    },
    AcceptanceScenario {
        id: AcceptanceScenarioId("session.pause"),
        summary: "Pausing stops at a safe boundary and keeps evidence",
        kind: ScenarioKind::Smoke,
        pty_test: Some("tests/recovery.rs::pause_stops_at_safe_boundary"),
        real_terminal_case: None,
        critical: false,
    },
    AcceptanceScenario {
        id: AcceptanceScenarioId("session.cancel_then_resume"),
        summary: "A cancelled session can be inspected and superseded",
        kind: ScenarioKind::Cancellation,
        pty_test: Some("tests/recovery.rs::cancelled_session_can_be_superseded"),
        real_terminal_case: Some("RT-10"),
        critical: true,
    },
    AcceptanceScenario {
        id: AcceptanceScenarioId("session.restart_then_resume"),
        summary: "Restarting the client resumes from durable state only",
        kind: ScenarioKind::Restart,
        pty_test: Some("tests/recovery.rs::restart_resumes_from_durable_state"),
        real_terminal_case: Some("RT-11"),
        critical: true,
    },
    AcceptanceScenario {
        id: AcceptanceScenarioId("session.history_is_read_only"),
        summary: "Opening history never starts execution",
        kind: ScenarioKind::Smoke,
        pty_test: Some("tests/recovery.rs::history_never_starts_execution"),
        real_terminal_case: None,
        critical: true,
    },
    AcceptanceScenario {
        id: AcceptanceScenarioId("session.draft_restoration"),
        summary: "An unsent draft is restored without leaking secrets",
        kind: ScenarioKind::Restart,
        pty_test: Some("tests/recovery.rs::draft_is_restored_without_secrets"),
        real_terminal_case: None,
        critical: true,
    },
    AcceptanceScenario {
        id: AcceptanceScenarioId("session.privacy_visible"),
        summary: "Local and remote inference state stays visible",
        kind: ScenarioKind::Smoke,
        pty_test: Some("tests/accessibility.rs::privacy_state_stays_visible"),
        real_terminal_case: Some("RT-18"),
        critical: true,
    },
    AcceptanceScenario {
        id: AcceptanceScenarioId("session.privacy_policy_blocked"),
        summary: "Policy that pins local-only inference refuses a switch to mixed",
        kind: ScenarioKind::Failure,
        pty_test: Some("tests/accessibility.rs::policy_refuses_switching_away_from_local_only"),
        real_terminal_case: None,
        critical: true,
    },
    AcceptanceScenario {
        id: AcceptanceScenarioId("provider.import_unparseable"),
        summary: "A pasted source with no provider configuration is refused, not guessed at",
        kind: ScenarioKind::Failure,
        pty_test: Some("tests/provider.rs::unparseable_import_is_refused"),
        real_terminal_case: None,
        critical: true,
    },
    AcceptanceScenario {
        id: AcceptanceScenarioId("provider.edit_requires_explicit_replace"),
        summary: "Saving over an existing profile requires a second explicit confirmation",
        kind: ScenarioKind::Failure,
        pty_test: Some("tests/provider.rs::overwriting_a_profile_requires_explicit_replace"),
        real_terminal_case: None,
        critical: true,
    },
    AcceptanceScenario {
        id: AcceptanceScenarioId("provider.remove_in_use"),
        summary: "Removing a provider an active session depends on is refused with a reason",
        kind: ScenarioKind::Failure,
        pty_test: Some("tests/provider.rs::removing_an_in_use_provider_is_refused"),
        real_terminal_case: None,
        critical: false,
    },
    AcceptanceScenario {
        id: AcceptanceScenarioId("model.pull_approve_refused"),
        summary: "The daemon refuses an approval for an action it is not awaiting",
        kind: ScenarioKind::Failure,
        pty_test: Some("tests/model.rs::daemon_refuses_approval_it_is_not_awaiting"),
        real_terminal_case: None,
        critical: true,
    },
    AcceptanceScenario {
        id: AcceptanceScenarioId("approval.reject_of_stale_action"),
        summary: "Rejecting an action that already resolved changes nothing",
        kind: ScenarioKind::Failure,
        pty_test: Some("tests/approval.rs::rejecting_a_resolved_action_changes_nothing"),
        real_terminal_case: None,
        critical: true,
    },
    AcceptanceScenario {
        id: AcceptanceScenarioId("review.rollback_unavailable"),
        summary: "Rollback with no agent-owned worktree is refused, not silently ignored",
        kind: ScenarioKind::Failure,
        pty_test: Some("tests/review.rs::rollback_without_agent_owned_work_is_refused"),
        real_terminal_case: None,
        critical: true,
    },
    AcceptanceScenario {
        id: AcceptanceScenarioId("skills.download_signature_failure"),
        summary: "A skill whose signature does not verify is never installed",
        kind: ScenarioKind::Failure,
        pty_test: Some("tests/evidence.rs::unverified_skill_signature_blocks_download"),
        real_terminal_case: None,
        critical: true,
    },
    // Recovery
    AcceptanceScenario {
        id: AcceptanceScenarioId("recovery.uncertain_session"),
        summary: "An uncertain session is never displayed as success",
        kind: ScenarioKind::Recovery,
        pty_test: Some("tests/recovery.rs::uncertain_session_is_not_success"),
        real_terminal_case: Some("RT-12"),
        critical: true,
    },
    AcceptanceScenario {
        id: AcceptanceScenarioId("recovery.lease_conflict"),
        summary: "A lease conflict explains itself and offers safe options",
        kind: ScenarioKind::Recovery,
        pty_test: Some("tests/recovery.rs::lease_conflict_offers_safe_options"),
        real_terminal_case: None,
        critical: true,
    },
    // Evidence
    AcceptanceScenario {
        id: AcceptanceScenarioId("evidence.trace_inspector"),
        summary: "Durable events can be inspected one at a time",
        kind: ScenarioKind::Smoke,
        pty_test: Some("tests/evidence.rs::durable_events_can_be_inspected"),
        real_terminal_case: None,
        critical: true,
    },
    AcceptanceScenario {
        id: AcceptanceScenarioId("evidence.action_explanation"),
        summary: "An action explains why it was allowed or blocked",
        kind: ScenarioKind::Smoke,
        pty_test: Some("tests/evidence.rs::action_explains_its_decision"),
        real_terminal_case: None,
        critical: true,
    },
    AcceptanceScenario {
        id: AcceptanceScenarioId("evidence.unavailable"),
        summary: "Missing evidence is reported as unavailable, not empty",
        kind: ScenarioKind::Failure,
        pty_test: Some("tests/evidence.rs::missing_evidence_reports_unavailable"),
        real_terminal_case: None,
        critical: true,
    },
    // Skills
    AcceptanceScenario {
        id: AcceptanceScenarioId("skills.browse"),
        summary: "Installed and discovered skills are distinguishable",
        kind: ScenarioKind::Smoke,
        pty_test: Some("tests/evidence.rs::skills_browse_distinguishes_installed"),
        real_terminal_case: None,
        critical: false,
    },
    AcceptanceScenario {
        id: AcceptanceScenarioId("skills.search"),
        summary: "A capability search returns bounded candidates",
        kind: ScenarioKind::Smoke,
        pty_test: Some("tests/evidence.rs::skill_search_returns_bounded_candidates"),
        real_terminal_case: None,
        critical: false,
    },
    AcceptanceScenario {
        id: AcceptanceScenarioId("skills.unavailable"),
        summary: "An unavailable skill registry is reported clearly",
        kind: ScenarioKind::Failure,
        pty_test: Some("tests/evidence.rs::unavailable_skill_registry_is_reported"),
        real_terminal_case: None,
        critical: false,
    },
    AcceptanceScenario {
        id: AcceptanceScenarioId("skills.download_requires_approval"),
        summary: "A skill download requires explicit approval",
        kind: ScenarioKind::Smoke,
        pty_test: Some("tests/evidence.rs::skill_download_requires_approval"),
        real_terminal_case: None,
        critical: true,
    },
    AcceptanceScenario {
        id: AcceptanceScenarioId("skills.install_requires_approval"),
        summary: "A skill install requires explicit approval",
        kind: ScenarioKind::Smoke,
        pty_test: Some("tests/evidence.rs::skill_install_requires_approval"),
        real_terminal_case: None,
        critical: true,
    },
    AcceptanceScenario {
        id: AcceptanceScenarioId("skills.qualification_failure"),
        summary: "A failed skill qualification blocks installation",
        kind: ScenarioKind::Failure,
        pty_test: Some("tests/evidence.rs::failed_qualification_blocks_install"),
        real_terminal_case: None,
        critical: false,
    },
    AcceptanceScenario {
        id: AcceptanceScenarioId("skills.block_publisher"),
        summary: "A blocked publisher is refused on later proposals",
        kind: ScenarioKind::Smoke,
        pty_test: Some("tests/evidence.rs::blocked_publisher_is_refused"),
        real_terminal_case: None,
        critical: false,
    },
    AcceptanceScenario {
        id: AcceptanceScenarioId("skills.mcp_approval"),
        summary: "An MCP tool call requires approval before running",
        kind: ScenarioKind::Smoke,
        pty_test: Some("tests/evidence.rs::mcp_tool_requires_approval"),
        real_terminal_case: None,
        critical: true,
    },
    AcceptanceScenario {
        id: AcceptanceScenarioId("skills.mcp_failure"),
        summary: "An MCP failure is explained and does not corrupt the session",
        kind: ScenarioKind::Failure,
        pty_test: Some("tests/evidence.rs::mcp_failure_is_explained"),
        real_terminal_case: None,
        critical: false,
    },
    // Settings and help
    AcceptanceScenario {
        id: AcceptanceScenarioId("settings.show"),
        summary: "Settings report the live provider and policy state",
        kind: ScenarioKind::Smoke,
        pty_test: Some("tests/startup.rs::settings_report_live_state"),
        real_terminal_case: None,
        critical: false,
    },
    AcceptanceScenario {
        id: AcceptanceScenarioId("help.command_palette"),
        summary: "The palette lists grouped actions with shortcuts",
        kind: ScenarioKind::Smoke,
        pty_test: Some("tests/startup.rs::palette_lists_grouped_actions"),
        real_terminal_case: Some("RT-14"),
        critical: true,
    },
    AcceptanceScenario {
        id: AcceptanceScenarioId("help.unavailable_explained"),
        summary: "Unavailable palette entries explain why",
        kind: ScenarioKind::Failure,
        pty_test: Some("tests/startup.rs::unavailable_palette_entries_explain_why"),
        real_terminal_case: Some("RT-14"),
        critical: true,
    },
];

// ── Query helpers used by the palette, help and footer ───────────

/// Actions matching a palette query, grouped by category in registry order.
pub fn filtered(query: &str) -> Vec<&'static UiActionDefinition> {
    let mut matched: Vec<&'static UiActionDefinition> = REGISTRY
        .iter()
        .filter(|action| action.matches(query))
        .collect();
    matched.sort_by_key(|action| (action.category, action.id));
    matched
}

/// The registered action a typed command resolves to, matching the longest
/// command entry first so `/model pull-approve` does not resolve to `/model`.
pub fn by_command(input: &str) -> Option<&'static UiActionDefinition> {
    let input = input.trim().trim_start_matches('/').to_ascii_lowercase();
    REGISTRY
        .iter()
        .flat_map(|action| {
            action
                .commands
                .iter()
                .map(move |command| (command.trim_start_matches('/').to_ascii_lowercase(), action))
        })
        .filter(|(command, _)| {
            input == *command
                || input
                    .strip_prefix(command.as_str())
                    .is_some_and(|rest| rest.starts_with(' '))
        })
        .max_by_key(|(command, _)| command.len())
        .map(|(_, action)| action)
}

/// The risk class of a typed command, when it maps to a registered action.
pub fn command_risk(input: &str) -> Option<UiRiskClass> {
    by_command(input).map(|action| action.risk)
}

/// Whether a typed command could start agent execution.
///
/// `None` means the command is not registered, which callers must treat as
/// "assume it can": an unrecognized command is not evidence of safety.
pub fn command_starts_execution(input: &str) -> Option<bool> {
    by_command(input).map(|action| action.starts_execution)
}

pub fn by_id(id: &str) -> Option<&'static UiActionDefinition> {
    REGISTRY.iter().find(|action| action.id.as_str() == id)
}

/// Contextual hints for the surface the user is currently looking at. Only
/// primary shortcuts for available actions in that context are advertised, so
/// the hint line never points at a dead end.
pub fn contextual_hints(context: ShortcutContext, state: &UiContext) -> Vec<String> {
    REGISTRY
        .iter()
        .filter_map(|action| {
            let shortcut = action
                .shortcuts
                .iter()
                .find(|shortcut| shortcut.primary && shortcut.context == context)?;
            action
                .availability(state)
                .is_available()
                .then(|| format!("{} {}", shortcut.keys, action.label))
        })
        .collect()
}

// ── Coverage report ──────────────────────────────────────────────

#[derive(Clone, Debug)]
pub struct CoverageRow {
    pub action: &'static str,
    pub category: &'static str,
    pub entry_points: Vec<String>,
    pub availability: String,
    pub risk: &'static str,
    pub pty_test: Option<&'static str>,
    pub real_terminal_case: Option<&'static str>,
    pub failure_test: Option<&'static str>,
    pub recovery_test: Option<&'static str>,
    pub status: CoverageStatus,
    pub gaps: Vec<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CoverageStatus {
    Covered,
    Incomplete,
}

impl CoverageStatus {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Covered => "covered",
            Self::Incomplete => "INCOMPLETE",
        }
    }
}

/// Build the acceptance-coverage report for the whole registry.
///
/// `known_commands` is the set of verbs the command dispatcher actually serves.
/// Passing it in keeps this module free of a duplicated command list while still
/// letting the report flag a command that no handler can run.
pub fn coverage(known_commands: &[&str]) -> Vec<CoverageRow> {
    REGISTRY
        .iter()
        .map(|action| {
            let mut gaps = Vec::new();
            if action.label.trim().is_empty() {
                gaps.push("missing label".to_owned());
            }
            if action.description.trim().is_empty() {
                gaps.push("missing description".to_owned());
            }
            if action.acceptance_scenarios.is_empty() {
                gaps.push("no acceptance scenario".to_owned());
            }
            if action.entry_points().is_empty() {
                gaps.push("no discovery surface".to_owned());
            }
            if let Some(verb) = action.handler.command_verb() {
                if !known_commands.contains(&verb) {
                    gaps.push(format!("handler verb `{verb}` has no dispatcher arm"));
                }
                if action.commands.is_empty() {
                    gaps.push("command handler with no command entry point".to_owned());
                }
            }
            for command in action.commands {
                let verb = command.trim_start_matches('/');
                let verb = verb.split_whitespace().next().unwrap_or(verb);
                if !known_commands.contains(&verb) {
                    gaps.push(format!("command `{command}` has no handler"));
                }
            }

            let mut pty_test = None;
            let mut real_terminal_case = None;
            let mut failure_test = None;
            let mut recovery_test = None;
            for id in action.acceptance_scenarios {
                let Some(scenario) = scenario(*id) else {
                    gaps.push(format!("unknown acceptance scenario `{id}`"));
                    continue;
                };
                if pty_test.is_none() {
                    pty_test = scenario.pty_test;
                }
                if real_terminal_case.is_none() {
                    real_terminal_case = scenario.real_terminal_case;
                }
                match scenario.kind {
                    ScenarioKind::Failure if failure_test.is_none() => {
                        failure_test = scenario.pty_test;
                    }
                    ScenarioKind::Recovery | ScenarioKind::Restart | ScenarioKind::Cancellation
                        if recovery_test.is_none() =>
                    {
                        recovery_test = scenario.pty_test;
                    }
                    _ => {}
                }
            }
            if pty_test.is_none() {
                gaps.push("no PTY test".to_owned());
            }
            if matches!(
                action.risk,
                UiRiskClass::Security | UiRiskClass::Destructive
            ) && failure_test.is_none()
            {
                gaps.push("high-risk action without a failure scenario".to_owned());
            }

            CoverageRow {
                action: action.id.as_str(),
                category: action.category.label(),
                entry_points: action.entry_points(),
                availability: action.availability.label(),
                risk: action.risk.label(),
                pty_test,
                real_terminal_case,
                failure_test,
                recovery_test,
                status: if gaps.is_empty() {
                    CoverageStatus::Covered
                } else {
                    CoverageStatus::Incomplete
                },
                gaps,
            }
        })
        .collect()
}

/// Commands the dispatcher serves that no registered action exposes. These are
/// orphan slash commands: reachable by typing, invisible in every discovery
/// surface.
pub fn orphan_commands(known_commands: &'static [&'static str]) -> Vec<&'static str> {
    known_commands
        .iter()
        .copied()
        .filter(|verb| !REGISTRY.iter().any(|action| action.serves_verb(verb)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command_palette::DISPATCH_COMMANDS;
    use std::collections::BTreeSet;

    fn context_with_everything() -> UiContext {
        UiContext {
            daemon_reachable: true,
            provider_configured: true,
            session_present: true,
            session_active: true,
            session_resumable: true,
            session_read_only: false,
            streaming: true,
            pending_approval: true,
            pending_model_pull: true,
            active_model_pull: true,
            repository_effects: true,
            validation_attention: true,
            recovery_required: true,
            local_model_provider: true,
            evidence_available: true,
            composer_has_text: true,
        }
    }

    /// The CI coverage gate. A registered user-facing action must be complete:
    /// labelled, explained, discoverable, availability-ruled, backed by a real
    /// handler, and proven by at least one acceptance scenario.
    #[test]
    fn coverage_gate() {
        let rows = coverage(DISPATCH_COMMANDS);
        let incomplete = rows
            .iter()
            .filter(|row| row.status == CoverageStatus::Incomplete)
            .map(|row| format!("{}: {}", row.action, row.gaps.join("; ")))
            .collect::<Vec<_>>();
        assert!(
            incomplete.is_empty(),
            "incomplete user-facing actions:\n{}",
            incomplete.join("\n")
        );
    }

    #[test]
    fn no_orphan_slash_commands() {
        let orphans = orphan_commands(DISPATCH_COMMANDS);
        assert!(
            orphans.is_empty(),
            "dispatcher verbs absent from every discovery surface: {orphans:?}"
        );
    }

    #[test]
    fn action_ids_and_scenario_ids_are_unique() {
        let ids: BTreeSet<_> = REGISTRY.iter().map(|action| action.id).collect();
        assert_eq!(ids.len(), REGISTRY.len(), "duplicate action id");
        let scenario_ids: BTreeSet<_> = SCENARIOS.iter().map(|scenario| scenario.id).collect();
        assert_eq!(
            scenario_ids.len(),
            SCENARIOS.len(),
            "duplicate acceptance scenario id"
        );
    }

    #[test]
    fn every_scenario_is_referenced_by_an_action() {
        let referenced: BTreeSet<_> = REGISTRY
            .iter()
            .flat_map(|action| action.acceptance_scenarios.iter().copied())
            .collect();
        let unreferenced = SCENARIOS
            .iter()
            .filter(|scenario| !referenced.contains(&scenario.id))
            .map(|scenario| scenario.id.as_str())
            .collect::<Vec<_>>();
        assert!(
            unreferenced.is_empty(),
            "scenarios declared but not attached to any action: {unreferenced:?}"
        );
    }

    #[test]
    fn every_category_is_represented() {
        for category in UiActionCategory::ALL {
            assert!(
                REGISTRY.iter().any(|action| action.category == *category),
                "category {category} has no registered action"
            );
        }
    }

    #[test]
    fn critical_scenarios_all_declare_a_pty_test() {
        let missing = SCENARIOS
            .iter()
            .filter(|scenario| scenario.critical && scenario.pty_test.is_none())
            .map(|scenario| scenario.id.as_str())
            .collect::<Vec<_>>();
        assert!(
            missing.is_empty(),
            "critical scenarios without a PTY test: {missing:?}"
        );
    }

    #[test]
    fn availability_reasons_are_stable_and_human_readable() {
        let empty = UiContext::default();
        let approve = by_id("approval.approve").unwrap();
        assert_eq!(
            approve.availability(&empty).reason(),
            Some("the daemon is not reachable")
        );
        let with_daemon = UiContext {
            daemon_reachable: true,
            ..UiContext::default()
        };
        assert_eq!(
            approve.availability(&with_daemon).reason(),
            Some("no action is pending")
        );
        assert!(approve
            .availability(&context_with_everything())
            .is_available());
    }

    #[test]
    fn read_only_history_blocks_task_submission_with_a_reason() {
        let state = UiContext {
            session_read_only: true,
            composer_has_text: true,
            ..context_with_everything()
        };
        let submit = by_id("task.submit").unwrap();
        assert_eq!(
            submit.availability(&state).reason(),
            Some("history is open read-only; start a new session first")
        );
    }

    #[test]
    fn contextual_hints_only_advertise_reachable_actions() {
        let empty = UiContext::default();
        let hints = contextual_hints(ShortcutContext::Approval, &empty);
        assert!(
            hints.is_empty(),
            "approval hints must stay hidden with no pending action: {hints:?}"
        );
        let pending = UiContext {
            daemon_reachable: true,
            pending_approval: true,
            ..UiContext::default()
        };
        let hints = contextual_hints(ShortcutContext::Approval, &pending);
        assert!(hints.contains(&"A Approve action".to_owned()), "{hints:?}");
        assert!(hints.contains(&"R Reject action".to_owned()), "{hints:?}");
        assert!(
            hints.contains(&"D Inspect pending action".to_owned()),
            "{hints:?}"
        );
    }

    #[test]
    fn common_actions_have_a_primary_shortcut_and_a_hint_surface() {
        for id in [
            "task.submit",
            "approval.approve",
            "approval.reject",
            "review.open_diff",
            "help.command_palette",
        ] {
            let action = by_id(id).unwrap();
            assert!(
                action.primary_shortcut().is_some(),
                "{id} must not depend on a memorized command only"
            );
        }
    }

    #[test]
    fn a_typed_command_resolves_to_its_registered_action() {
        assert_eq!(
            by_command("/diff").map(|a| a.id.as_str()),
            Some("review.open_diff")
        );
        assert_eq!(
            by_command("/model pull-approve").map(|a| a.id.as_str()),
            Some("model.pull_approve"),
            "the longest matching command must win over the /model prefix"
        );
        assert_eq!(
            by_command("/provider remove local").map(|a| a.id.as_str()),
            Some("provider.remove")
        );
        assert!(by_command("/notacommand").is_none());
    }

    /// The read-only-history invariant, stated on the registry itself.
    #[test]
    fn only_execution_starting_actions_are_marked_as_such() {
        for id in [
            "task.submit",
            "session.resume",
            "approval.approve",
            "model.pull_approve",
        ] {
            assert!(
                by_id(id).unwrap().starts_execution,
                "{id} can cause the agent to run and must be marked"
            );
        }
        for id in [
            "review.open_diff",
            "review.rollback",
            "session.list",
            "session.pause",
            "approval.reject",
            "evidence.trace_inspector",
            "help.command_palette",
        ] {
            assert!(
                !by_id(id).unwrap().starts_execution,
                "{id} does not start execution and must stay usable in history"
            );
        }
    }

    #[test]
    fn every_security_action_that_grants_authority_starts_execution() {
        for action in REGISTRY {
            if action.id.as_str() == "approval.approve" {
                assert!(action.starts_execution);
            }
        }
        // Rejection is security-classed but can never start anything.
        assert!(!by_id("approval.reject").unwrap().starts_execution);
    }

    #[test]
    fn an_unregistered_command_is_never_reported_as_safe() {
        assert_eq!(command_starts_execution("/notacommand"), None);
    }

    #[test]
    fn command_risk_distinguishes_read_only_commands_from_state_changes() {
        assert_eq!(command_risk("/diff"), Some(UiRiskClass::Safe));
        assert_eq!(command_risk("/sessions"), Some(UiRiskClass::Safe));
        assert_eq!(command_risk("/help"), Some(UiRiskClass::Safe));
        assert_eq!(command_risk("/rollback"), Some(UiRiskClass::Destructive));
        assert_eq!(command_risk("/approve"), Some(UiRiskClass::Security));
        assert_eq!(command_risk("/new"), Some(UiRiskClass::Elevated));
    }

    #[test]
    fn palette_query_matches_label_description_command_and_category() {
        assert!(filtered("approve")
            .iter()
            .any(|action| action.id.as_str() == "approval.approve"));
        assert!(filtered("/diff")
            .iter()
            .any(|action| action.id.as_str() == "review.open_diff"));
        assert!(filtered("provider")
            .iter()
            .any(|action| action.category == UiActionCategory::Provider));
        assert_eq!(filtered("").len(), REGISTRY.len());
    }

    #[test]
    fn security_and_destructive_actions_declare_a_failure_scenario() {
        let rows = coverage(DISPATCH_COMMANDS);
        for row in rows
            .iter()
            .filter(|row| matches!(row.risk, "security" | "destructive"))
        {
            assert!(
                row.failure_test.is_some(),
                "{} is {} but has no failure scenario",
                row.action,
                row.risk
            );
        }
    }
}
