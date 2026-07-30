//! The exact pending action, derived from durable events.
//!
//! The approval surface must show the action the daemon actually holds, not a
//! summary the client invented. Everything here deserializes the durable event
//! payload back into the runtime's own types and computes the digest with
//! [`purrcode_runtime_core::ProposedAction::digest`], so the value shown is the
//! same value the daemon records in `approval_recorded.action_digest`. There is
//! no second digest implementation to drift.
//!
//! This module grants no authority. Approving still calls the daemon endpoint,
//! which re-checks its own `AwaitingApproval` boundary; the client can only
//! refuse to send a decision it cannot vouch for.

use purrcode_runtime_core::{ActionConstraints, ProposedAction};
use serde_json::Value;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ActionKind {
    Command,
    RepositoryRead,
    WriteFile,
    DeleteFile,
    ExternalTool,
}

impl ActionKind {
    const fn of(action: &ProposedAction) -> Self {
        match action {
            ProposedAction::Command(_) => Self::Command,
            ProposedAction::RepositoryRead(_) => Self::RepositoryRead,
            ProposedAction::WriteFile(_) => Self::WriteFile,
            ProposedAction::DeleteFile(_) => Self::DeleteFile,
            ProposedAction::ExternalTool(_) => Self::ExternalTool,
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::Command => "Run a command",
            Self::RepositoryRead => "Read from the repository",
            Self::WriteFile => "Write a file",
            Self::DeleteFile => "Delete a file",
            Self::ExternalTool => "Call an external tool",
        }
    }
}

/// Risk class shown on the approval surface. Derived from what the action can
/// reach, not from the model's opinion of it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ActionRisk {
    Read,
    Write,
    Delete,
    Network,
    External,
}

impl ActionRisk {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Read => "read-only",
            Self::Write => "modifies files",
            Self::Delete => "deletes files",
            Self::Network => "reaches the network",
            Self::External => "calls an external service",
        }
    }
}

/// Why the approval surface may not be trusted as current.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Staleness {
    #[default]
    Current,
    /// The action shown is not the action the daemon is waiting on.
    ActionChanged,
    /// The digest of the displayed action does not match the recorded digest.
    DigestMismatch,
    /// The boundary was resolved while the surface was open.
    AlreadyResolved,
}

impl Staleness {
    pub const fn is_current(self) -> bool {
        matches!(self, Self::Current)
    }

    pub const fn warning(self) -> Option<&'static str> {
        match self {
            Self::Current => None,
            Self::ActionChanged => Some(
                "This is no longer the action PurrCode is waiting on. Refresh before deciding.",
            ),
            Self::DigestMismatch => Some(
                "The displayed action does not match its recorded digest. Approval is blocked.",
            ),
            Self::AlreadyResolved => {
                Some("This action was already decided. Nothing further will run.")
            }
        }
    }
}

/// A complete, self-describing approval request.
#[derive(Clone, Debug)]
pub struct ApprovalRequest {
    pub action_id: String,
    /// Digest over the exact action and its constraints, computed with the
    /// runtime's own function.
    pub digest: String,
    pub kind: ActionKind,
    /// One-line description of the operation, e.g. `cargo test --all`.
    pub operation: String,
    /// Why approval is required, as recorded by the judgment.
    pub reason: String,
    pub risk: Vec<ActionRisk>,
    /// Paths the action may affect.
    pub affected_paths: Vec<String>,
    pub constraints: ActionConstraints,
    pub staleness: Staleness,
}

impl ApprovalRequest {
    /// Find the pending approval boundary in the durable event list.
    ///
    /// Returns `None` when no action is awaiting approval; the approval surface
    /// is never shown speculatively.
    pub fn from_events(events: &[Value]) -> Option<Self> {
        let mut proposals: Vec<(String, Value)> = Vec::new();
        let mut pending: Option<(String, Value, ActionConstraints, String)> = None;
        let mut resolved = false;

        for event in events {
            let name = event.get("event").and_then(Value::as_str).unwrap_or("");
            let data = event.get("data").unwrap_or(&Value::Null);
            let action_id = data.get("action_id").and_then(Value::as_str);
            match name {
                "action_proposed" => {
                    if let (Some(action_id), Some(action)) = (action_id, data.get("action")) {
                        proposals.push((action_id.to_owned(), action.clone()));
                    }
                }
                "judgment_recorded" => {
                    let decision = data
                        .pointer("/decision/decision")
                        .and_then(Value::as_str)
                        .unwrap_or("");
                    let Some(action_id) = action_id else { continue };
                    if decision == "require_approval" {
                        let Some(action) = proposals
                            .iter()
                            .rev()
                            .find(|(id, _)| id == action_id)
                            .map(|(_, action)| action.clone())
                        else {
                            continue;
                        };
                        let constraints =
                            data.pointer("/decision/details/constraints")
                                .and_then(|value| {
                                    serde_json::from_value::<ActionConstraints>(value.clone()).ok()
                                });
                        let reason = data
                            .pointer("/decision/details/reason")
                            .and_then(Value::as_str)
                            .unwrap_or("PawGate requires explicit approval for this action")
                            .to_owned();
                        // Without constraints the client cannot describe the
                        // action's scope, and an approval surface that cannot
                        // state the scope must not be shown as authoritative.
                        if let Some(constraints) = constraints {
                            pending = Some((action_id.to_owned(), action, constraints, reason));
                            resolved = false;
                        }
                    } else if pending
                        .as_ref()
                        .is_some_and(|(id, _, _, _)| id == action_id)
                    {
                        pending = None;
                    }
                }
                "approval_recorded"
                | "approval_rejected"
                | "authorization_persisted"
                | "execution_started" => {
                    if pending
                        .as_ref()
                        .is_some_and(|(id, _, _, _)| Some(id.as_str()) == action_id)
                    {
                        resolved = true;
                    }
                }
                "session_completed" | "session_failed" | "session_cancelled" => resolved = true,
                _ => {}
            }
        }

        let (action_id, action_json, constraints, reason) = pending?;
        if resolved {
            return None;
        }
        let action: ProposedAction = serde_json::from_value(action_json.clone()).ok()?;
        let digest = action.digest(&constraints).ok()?;
        Some(Self {
            action_id,
            digest,
            kind: ActionKind::of(&action),
            operation: describe(&action),
            reason,
            risk: risk_of(&action, &constraints),
            affected_paths: affected_paths(&action, &constraints),
            constraints,
            staleness: Staleness::Current,
        })
    }

    /// Compare this request against the daemon's current boundary.
    ///
    /// `daemon_action_id` is what the daemon reports it is waiting on. A
    /// mismatch marks the surface stale so the decision keys refuse to fire.
    pub fn checked_against(mut self, daemon_action_id: Option<&str>) -> Self {
        self.staleness = match daemon_action_id {
            Some(id) if id == self.action_id => Staleness::Current,
            Some(_) => Staleness::ActionChanged,
            None => Staleness::AlreadyResolved,
        };
        self
    }

    /// Recompute the digest from the displayed action and constraints and mark
    /// the surface stale if it disagrees with `recorded`.
    pub fn verified_digest(mut self, recorded: &str) -> Self {
        if !recorded.is_empty() && recorded != self.digest {
            self.staleness = Staleness::DigestMismatch;
        }
        self
    }

    /// Whether a decision may be submitted. A stale surface blocks approval
    /// entirely rather than sending a decision about the wrong action.
    pub fn can_decide(&self) -> bool {
        self.staleness.is_current()
    }

    /// Risk in one line. `separator` comes from the active symbol set, because a
    /// terminal without Unicode cannot render a middle dot.
    pub fn risk_summary_with(&self, separator: &str) -> String {
        if self.risk.is_empty() {
            return ActionRisk::Read.label().to_owned();
        }
        self.risk
            .iter()
            .map(|risk| risk.label())
            .collect::<Vec<_>>()
            .join(separator)
    }

    pub fn risk_summary(&self) -> String {
        self.risk_summary_with(" · ")
    }

    /// Read/write/network scope in one line.
    pub fn scope_summary_with(&self, separator: &str) -> String {
        let write = if self.constraints.allowed_write_globs.is_empty() {
            "no writes".to_owned()
        } else {
            format!(
                "writes: {}",
                self.constraints.allowed_write_globs.join(", ")
            )
        };
        let network = if self.constraints.network {
            "network allowed"
        } else {
            "no network"
        };
        format!("read: repository{separator}{write}{separator}{network}")
    }

    pub fn scope_summary(&self) -> String {
        self.scope_summary_with(" · ")
    }

    pub fn limits_summary_with(&self, separator: &str) -> String {
        format!(
            "{} s timeout{separator}{} output bytes{separator}at most {} changed file(s)",
            self.constraints.timeout_seconds,
            self.constraints.maximum_output_bytes,
            self.constraints.maximum_changed_files
        )
    }

    pub fn limits_summary(&self) -> String {
        self.limits_summary_with(" · ")
    }
}

fn describe(action: &ProposedAction) -> String {
    match action {
        ProposedAction::Command(command) => {
            let arguments = command.arguments.join(" ");
            let program = command.program.display().to_string();
            if arguments.is_empty() {
                program
            } else {
                format!("{program} {arguments}")
            }
        }
        ProposedAction::RepositoryRead(read) => format!("{read:?}"),
        ProposedAction::WriteFile(write) => {
            format!("write {}", write.path.display())
        }
        ProposedAction::DeleteFile(delete) => format!("delete {}", delete.path.display()),
        ProposedAction::ExternalTool(tool) => {
            format!("{} / {}", tool.server_id, tool.tool_name)
        }
    }
}

fn risk_of(action: &ProposedAction, constraints: &ActionConstraints) -> Vec<ActionRisk> {
    let mut risk = Vec::new();
    match action {
        ProposedAction::WriteFile(_) => risk.push(ActionRisk::Write),
        ProposedAction::DeleteFile(_) => risk.push(ActionRisk::Delete),
        ProposedAction::ExternalTool(_) => risk.push(ActionRisk::External),
        ProposedAction::Command(_) => {
            if constraints.maximum_changed_files > 0 || !constraints.allowed_write_globs.is_empty()
            {
                risk.push(ActionRisk::Write);
            } else {
                risk.push(ActionRisk::Read);
            }
        }
        ProposedAction::RepositoryRead(_) => risk.push(ActionRisk::Read),
    }
    if constraints.network {
        risk.push(ActionRisk::Network);
    }
    risk
}

fn affected_paths(action: &ProposedAction, constraints: &ActionConstraints) -> Vec<String> {
    let mut paths = match action {
        ProposedAction::WriteFile(write) => vec![write.path.display().to_string()],
        ProposedAction::DeleteFile(delete) => vec![delete.path.display().to_string()],
        _ => Vec::new(),
    };
    if paths.is_empty() && !constraints.allowed_write_globs.is_empty() {
        paths.extend(constraints.allowed_write_globs.iter().cloned());
    }
    if paths.is_empty() {
        paths.push(format!(
            "{} (working directory, read-only)",
            constraints.working_directory.display()
        ));
    }
    paths
}

#[cfg(test)]
mod tests {
    use super::*;
    use purrcode_runtime_core::{CommandAction, WriteFileAction};
    use serde_json::json;
    use std::path::PathBuf;

    fn constraints() -> ActionConstraints {
        ActionConstraints {
            working_directory: PathBuf::from("/repo"),
            network: false,
            timeout_seconds: 120,
            maximum_output_bytes: 1_048_576,
            allowed_write_globs: vec!["src/**".into()],
            maximum_changed_files: 3,
        }
    }

    fn write_action() -> ProposedAction {
        ProposedAction::WriteFile(WriteFileAction {
            path: PathBuf::from("src/lib.rs"),
            content: "fn main() {}".into(),
            expected_digest: None,
        })
    }

    fn boundary(action: &ProposedAction, constraints: &ActionConstraints) -> Vec<Value> {
        vec![
            json!({"event":"action_proposed","data":{"action_id":"action-1","action":action}}),
            json!({"event":"judgment_recorded","data":{
                "action_id":"action-1",
                "decision":{"decision":"require_approval","details":{
                    "reason":"writes outside the plan's declared scope",
                    "constraints":constraints,
                }}
            }}),
        ]
    }

    #[test]
    fn pending_boundary_is_described_completely() {
        let request = ApprovalRequest::from_events(&boundary(&write_action(), &constraints()))
            .expect("a require_approval judgment must produce a request");
        assert_eq!(request.action_id, "action-1");
        assert_eq!(request.kind, ActionKind::WriteFile);
        assert_eq!(request.operation, "write src/lib.rs");
        assert_eq!(request.reason, "writes outside the plan's declared scope");
        assert_eq!(request.affected_paths, vec!["src/lib.rs"]);
        assert!(request.risk_summary().contains("modifies files"));
        assert!(request.scope_summary().contains("src/**"));
        assert!(request.scope_summary().contains("no network"));
        assert!(request.limits_summary().contains("120 s timeout"));
        assert!(request
            .limits_summary()
            .contains("at most 3 changed file(s)"));
        assert!(request.can_decide());
    }

    #[test]
    fn the_displayed_digest_is_the_runtime_digest() {
        let action = write_action();
        let constraints = constraints();
        let request = ApprovalRequest::from_events(&boundary(&action, &constraints)).unwrap();
        assert_eq!(
            request.digest,
            action.digest(&constraints).unwrap(),
            "the surface must show the same digest the daemon records"
        );
        assert!(!request.digest.is_empty());
    }

    #[test]
    fn no_pending_action_yields_no_surface() {
        assert!(ApprovalRequest::from_events(&[]).is_none());
        assert!(
            ApprovalRequest::from_events(&[
                json!({"event":"action_proposed","data":{"action_id":"a","action":write_action()}}),
            ])
            .is_none(),
            "a proposal alone is not an approval boundary"
        );
    }

    #[test]
    fn an_allowed_action_never_produces_an_approval_surface() {
        let events = vec![
            json!({"event":"action_proposed","data":{"action_id":"a","action":write_action()}}),
            json!({"event":"judgment_recorded","data":{"action_id":"a","decision":{"decision":"allow"}}}),
        ];
        assert!(ApprovalRequest::from_events(&events).is_none());
    }

    #[test]
    fn a_resolved_boundary_disappears() {
        for resolution in [
            json!({"event":"approval_recorded","data":{"action_id":"action-1","action_digest":"d"}}),
            json!({"event":"approval_rejected","data":{"action_id":"action-1","reason":"no"}}),
            json!({"event":"execution_started","data":{"action_id":"action-1"}}),
        ] {
            let mut events = boundary(&write_action(), &constraints());
            events.push(resolution.clone());
            assert!(
                ApprovalRequest::from_events(&events).is_none(),
                "boundary survived {resolution}"
            );
        }
    }

    #[test]
    fn a_denied_action_does_not_leave_a_pending_boundary() {
        let mut events = boundary(&write_action(), &constraints());
        events.push(json!({"event":"judgment_recorded","data":{
            "action_id":"action-1",
            "decision":{"decision":"deny","details":{"reason":"unsafe"}}
        }}));
        assert!(ApprovalRequest::from_events(&events).is_none());
    }

    #[test]
    fn a_changed_daemon_action_marks_the_surface_stale_and_blocks_deciding() {
        let request = ApprovalRequest::from_events(&boundary(&write_action(), &constraints()))
            .unwrap()
            .checked_against(Some("a-different-action"));
        assert_eq!(request.staleness, Staleness::ActionChanged);
        assert!(!request.can_decide());
        assert!(request.staleness.warning().unwrap().contains("Refresh"));
    }

    #[test]
    fn a_resolved_daemon_boundary_marks_the_surface_stale() {
        let request = ApprovalRequest::from_events(&boundary(&write_action(), &constraints()))
            .unwrap()
            .checked_against(None);
        assert_eq!(request.staleness, Staleness::AlreadyResolved);
        assert!(!request.can_decide());
    }

    #[test]
    fn a_digest_mismatch_blocks_approval_visibly() {
        let request = ApprovalRequest::from_events(&boundary(&write_action(), &constraints()))
            .unwrap()
            .verified_digest("0000000000000000000000000000000000000000000000000000000000000000");
        assert_eq!(request.staleness, Staleness::DigestMismatch);
        assert!(!request.can_decide());
        assert!(request
            .staleness
            .warning()
            .unwrap()
            .contains("does not match"));
    }

    #[test]
    fn a_matching_digest_keeps_the_surface_current() {
        let request =
            ApprovalRequest::from_events(&boundary(&write_action(), &constraints())).unwrap();
        let digest = request.digest.clone();
        let request = request
            .verified_digest(&digest)
            .checked_against(Some("action-1"));
        assert!(request.can_decide());
        assert_eq!(request.staleness, Staleness::Current);
    }

    #[test]
    fn changing_the_action_changes_the_digest() {
        let constraints = constraints();
        let first = ApprovalRequest::from_events(&boundary(&write_action(), &constraints))
            .unwrap()
            .digest;
        let other = ProposedAction::WriteFile(WriteFileAction {
            path: PathBuf::from("src/lib.rs"),
            content: "fn main() { panic!() }".into(),
            expected_digest: None,
        });
        let second = ApprovalRequest::from_events(&boundary(&other, &constraints))
            .unwrap()
            .digest;
        assert_ne!(
            first, second,
            "the digest must bind the exact action content"
        );
    }

    #[test]
    fn changing_the_constraints_changes_the_digest() {
        let action = write_action();
        let first = ApprovalRequest::from_events(&boundary(&action, &constraints()))
            .unwrap()
            .digest;
        let widened = ActionConstraints {
            network: true,
            ..constraints()
        };
        let second = ApprovalRequest::from_events(&boundary(&action, &widened))
            .unwrap()
            .digest;
        assert_ne!(
            first, second,
            "the digest must bind the granted scope, not only the action"
        );
    }

    #[test]
    fn a_network_command_reports_network_risk_and_scope() {
        let action = ProposedAction::Command(CommandAction {
            program: PathBuf::from("curl"),
            arguments: vec!["https://example.invalid".into()],
            working_directory: PathBuf::from("/repo"),
            environment: Default::default(),
        });
        let constraints = ActionConstraints {
            network: true,
            allowed_write_globs: Vec::new(),
            maximum_changed_files: 0,
            ..constraints()
        };
        let request = ApprovalRequest::from_events(&boundary(&action, &constraints)).unwrap();
        assert_eq!(request.kind, ActionKind::Command);
        assert_eq!(request.operation, "curl https://example.invalid");
        assert!(request.risk_summary().contains("reaches the network"));
        assert!(request.scope_summary().contains("network allowed"));
        assert!(request.scope_summary().contains("no writes"));
    }

    #[test]
    fn a_delete_reports_delete_risk_and_the_exact_path() {
        let action = ProposedAction::DeleteFile(purrcode_runtime_core::DeleteFileAction {
            path: PathBuf::from("src/old.rs"),
            expected_digest: "abc123".into(),
        });
        let request = ApprovalRequest::from_events(&boundary(&action, &constraints())).unwrap();
        assert_eq!(request.kind, ActionKind::DeleteFile);
        assert!(request.risk_summary().contains("deletes files"));
        assert_eq!(request.affected_paths, vec!["src/old.rs"]);
    }

    #[test]
    fn a_read_only_action_still_names_a_scope() {
        let action = ProposedAction::Command(CommandAction {
            program: PathBuf::from("git"),
            arguments: vec!["status".into()],
            working_directory: PathBuf::from("/repo"),
            environment: Default::default(),
        });
        let constraints = ActionConstraints::read_only(PathBuf::from("/repo"));
        let request = ApprovalRequest::from_events(&boundary(&action, &constraints)).unwrap();
        assert!(request.risk_summary().contains("read-only"));
        assert!(!request.affected_paths.is_empty());
        assert!(request.affected_paths[0].contains("/repo"));
    }

    #[test]
    fn a_judgment_without_constraints_does_not_produce_an_authoritative_surface() {
        let events = vec![
            json!({"event":"action_proposed","data":{"action_id":"a","action":write_action()}}),
            json!({"event":"judgment_recorded","data":{
                "action_id":"a",
                "decision":{"decision":"require_approval","details":{"reason":"why"}}
            }}),
        ];
        assert!(
            ApprovalRequest::from_events(&events).is_none(),
            "a surface that cannot state the granted scope must not be shown"
        );
    }

    #[test]
    fn the_latest_boundary_wins_when_an_action_is_superseded() {
        let mut events = boundary(&write_action(), &constraints());
        let replacement = ProposedAction::WriteFile(WriteFileAction {
            path: PathBuf::from("src/other.rs"),
            content: "x".into(),
            expected_digest: None,
        });
        events.push(
            json!({"event":"action_proposed","data":{"action_id":"action-2","action":replacement}}),
        );
        events.push(json!({"event":"judgment_recorded","data":{
            "action_id":"action-2",
            "decision":{"decision":"require_approval","details":{
                "reason":"second","constraints":constraints(),
            }}
        }}));
        let request = ApprovalRequest::from_events(&events).unwrap();
        assert_eq!(request.action_id, "action-2");
        assert_eq!(request.operation, "write src/other.rs");
    }

    #[test]
    fn summaries_use_the_separator_they_are_given() {
        let request =
            ApprovalRequest::from_events(&boundary(&write_action(), &constraints())).unwrap();
        for summary in [
            request.risk_summary_with(" | "),
            request.scope_summary_with(" | "),
            request.limits_summary_with(" | "),
        ] {
            assert!(
                summary.is_ascii(),
                "an ASCII separator must yield an ASCII summary: {summary}"
            );
        }
    }

    #[test]
    fn every_staleness_state_other_than_current_carries_a_warning() {
        for state in [
            Staleness::ActionChanged,
            Staleness::DigestMismatch,
            Staleness::AlreadyResolved,
        ] {
            assert!(state.warning().is_some(), "{state:?} must explain itself");
            assert!(!state.is_current());
        }
        assert!(Staleness::Current.warning().is_none());
    }

    #[test]
    fn every_action_kind_and_risk_has_a_label() {
        for kind in [
            ActionKind::Command,
            ActionKind::RepositoryRead,
            ActionKind::WriteFile,
            ActionKind::DeleteFile,
            ActionKind::ExternalTool,
        ] {
            assert!(!kind.label().is_empty());
        }
        for risk in [
            ActionRisk::Read,
            ActionRisk::Write,
            ActionRisk::Delete,
            ActionRisk::Network,
            ActionRisk::External,
        ] {
            assert!(!risk.label().is_empty());
        }
    }
}
