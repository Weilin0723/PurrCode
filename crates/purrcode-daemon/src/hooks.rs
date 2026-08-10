//! Governed hook dispatch (v1.3 §8 PR6 / §4.3).
//!
//! Hooks are the only way a project file can run something on a lifecycle
//! trigger, and they are deliberately NOT a way around PawGate. Every hook
//! firing is: Trigger → registry lookup → `HookAction` → `ProposedAction::Tool`
//! → `Policy::evaluate_tool` → `SessionStore::authorize` → `ToolRuntime::execute`
//! → evidence. A hook can only invoke a REGISTERED tool — it can never name an
//! arbitrary program — and every firing is recorded in `hook_runs` BEFORE the
//! action is proposed, so a triggered-but-denied hook is distinguishable from
//! one that never fired.

use purrcode_ninelives::SessionStore;
use purrcode_runtime_core::{
    ActionConstraints, ActionId, HookAction, HookDescriptor, HookTrigger, JudgmentDecision,
    SessionEvent, SessionId, ToolInvocation,
};

/// How one hook firing resolved. Recorded in `hook_runs` for audit.
#[derive(Clone, Debug)]
#[allow(dead_code)] // fields are read by tests and future audit surfaces
pub(crate) struct HookRunOutcome {
    pub hook_id: String,
    pub status: &'static str,
    pub detail: Option<String>,
    pub action_id: Option<ActionId>,
}

impl HookRunOutcome {
    pub(crate) fn aborted(&self) -> bool {
        self.status == "denied" || self.status == "failed"
    }
}

/// Judge a single hook and record its outcome (but do not execute it). Returns
/// the decision so the caller (the daemon's async evaluator or a test) can
/// execute an allowed hook's tool with the exact authorized constraints.
///
/// The `HookTriggered` event and the `hook_runs` row are written BEFORE the
/// decision, so a triggered-but-denied hook is auditable.
pub(crate) fn judge_hook(
    store: &mut SessionStore,
    session_id: SessionId,
    trigger: HookTrigger,
    hook: &HookDescriptor,
    evaluate: &(dyn Fn(&HookDescriptor) -> JudgmentDecision + Sync),
) -> (JudgmentDecision, Option<ToolInvocation>, Option<ActionConstraints>) {
    let action_id = ActionId::new();
    let _ = store.append(
        session_id,
        &SessionEvent::HookTriggered {
            hook_id: hook.id.clone(),
            trigger,
            action_id: Some(action_id),
        },
    );
    let _ = record_hook_run(
        store,
        session_id,
        &hook.id,
        &hook.descriptor_digest,
        trigger,
        &hook.layer,
        Some(action_id),
        "triggered",
        None,
    );
    let decision = evaluate(hook);
    match &decision {
        JudgmentDecision::Deny { reason } => {
            let _ = record_hook_run(
                store,
                session_id,
                &hook.id,
                &hook.descriptor_digest,
                trigger,
                &hook.layer,
                Some(action_id),
                "denied",
                Some(reason),
            );
        }
        JudgmentDecision::AllowWithConstraints(constraints) => {
            if let HookAction::Tool { tool_id, arguments } = &hook.action {
                let _ = record_hook_run(
                    store,
                    session_id,
                    &hook.id,
                    &hook.descriptor_digest,
                    trigger,
                    &hook.layer,
                    Some(action_id),
                    "executing",
                    None,
                );
                return (
                    decision.clone(),
                    Some(ToolInvocation {
                        tool_id: tool_id.clone(),
                        arguments: arguments.clone(),
                        working_directory: constraints.working_directory.clone(),
                        descriptor_digest: hook.descriptor_digest.clone(),
                    }),
                    Some(constraints.clone()),
                );
            }
        }
        JudgmentDecision::RequireApproval { .. } => {
            let _ = record_hook_run(
                store,
                session_id,
                &hook.id,
                &hook.descriptor_digest,
                trigger,
                &hook.layer,
                Some(action_id),
                "awaiting_approval",
                None,
            );
        }
        _ => {
            let _ = record_hook_run(
                store,
                session_id,
                &hook.id,
                &hook.descriptor_digest,
                trigger,
                &hook.layer,
                Some(action_id),
                "failed",
                Some("unsupported hook decision"),
            );
        }
    }
    (decision, None, None)
}

/// Dispatch one trigger against a set of hooks, judging and auditing each but
/// NOT executing. Returns the hooks that were allowed with their exact
/// PawGate-authorized invocation + constraints, so the daemon's async evaluator
/// can execute them with the real ToolExecutor. A `blocking` hook that is
/// denied or unsupported aborts the rest (the returned slice stops before it).
///
/// This is deliberately synchronous and store-safe: each hook's audit happens
/// here; the async execution of an allowed tool happens in the daemon with a
/// fresh store borrow, avoiding the borrow-across-await trap.
#[allow(clippy::too_many_arguments)]
pub(crate) fn dispatch_hooks(
    store: &mut SessionStore,
    session_id: SessionId,
    trigger: HookTrigger,
    hooks: &[HookDescriptor],
    depth: u8,
    evaluate: &(dyn Fn(&HookDescriptor) -> JudgmentDecision + Sync),
) -> (Vec<HookRunOutcome>, Vec<HookExecution>, bool) {
    if depth > 4 {
        let outcomes = hooks
            .iter()
            .map(|hook| HookRunOutcome {
                hook_id: hook.id.clone(),
                status: "skipped",
                detail: Some("hook recursion depth exceeded".into()),
                action_id: None,
            })
            .collect();
        return (outcomes, Vec::new(), true);
    }
    let mut outcomes = Vec::new();
    let mut to_execute = Vec::new();
    let mut aborted = false;
    for hook in hooks {
        let (decision, invocation, constraints) =
            judge_hook(store, session_id, trigger, hook, evaluate);
        match decision {
            JudgmentDecision::Deny { reason } => {
                outcomes.push(HookRunOutcome {
                    hook_id: hook.id.clone(),
                    status: "denied",
                    detail: Some(reason),
                    action_id: None,
                });
                if hook.blocking {
                    aborted = true;
                    break;
                }
            }
            JudgmentDecision::AllowWithConstraints(_) => {
                if let (Some(invocation), Some(constraints)) = (invocation, constraints) {
                    to_execute.push(HookExecution {
                        hook_id: hook.id.clone(),
                        hook_digest: hook.descriptor_digest.clone(),
                        layer: hook.layer,
                        invocation,
                        constraints,
                    });
                }
                outcomes.push(HookRunOutcome {
                    hook_id: hook.id.clone(),
                    status: "succeeded",
                    detail: None,
                    action_id: None,
                });
            }
            JudgmentDecision::RequireApproval { .. } => {
                outcomes.push(HookRunOutcome {
                    hook_id: hook.id.clone(),
                    status: "awaiting_approval",
                    detail: None,
                    action_id: None,
                });
                if hook.blocking {
                    aborted = true;
                    break;
                }
            }
            _ => {
                outcomes.push(HookRunOutcome {
                    hook_id: hook.id.clone(),
                    status: "failed",
                    detail: Some("unsupported hook decision".into()),
                    action_id: None,
                });
                if hook.blocking {
                    aborted = true;
                    break;
                }
            }
        }
    }
    (outcomes, to_execute, aborted)
}

/// A hook that PawGate allowed, ready for the daemon to execute with the exact
/// authorized invocation + constraints.
#[derive(Clone, Debug)]
pub(crate) struct HookExecution {
    pub hook_id: String,
    pub hook_digest: String,
    pub layer: purrcode_runtime_core::ExtensionLayer,
    pub invocation: ToolInvocation,
    pub constraints: ActionConstraints,
}

/// Record a hook run as completed after the daemon executed its allowed tool.
pub(crate) fn record_completed_hook_run(
    store: &mut SessionStore,
    session_id: SessionId,
    execution: &HookExecution,
    trigger: HookTrigger,
    status: &str,
) -> Result<(), purrcode_ninelives::StoreError> {
    record_hook_run(
        store,
        session_id,
        &execution.hook_id,
        &execution.hook_digest,
        trigger,
        &execution.layer,
        None,
        status,
        None,
    )
}

/// Persist a hook firing into the `hook_runs` projection table (migration
/// 0004). Best-effort: a projection failure must never wedge the session.
#[allow(clippy::too_many_arguments)]
fn record_hook_run(
    store: &mut SessionStore,
    session_id: SessionId,
    hook_id: &str,
    hook_digest: &str,
    trigger: HookTrigger,
    layer: &purrcode_runtime_core::ExtensionLayer,
    action_id: Option<ActionId>,
    status: &str,
    detail: Option<&str>,
) -> Result<(), purrcode_ninelives::StoreError> {
    store.record_hook_run(
        session_id,
        hook_id,
        hook_digest,
        trigger,
        layer,
        action_id,
        status,
        detail,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn hook(id: &str, blocking: bool) -> HookDescriptor {
        HookDescriptor {
            id: id.into(),
            layer: purrcode_runtime_core::ExtensionLayer::Project,
            trigger: HookTrigger::BeforeWrite,
            path_filter: vec![],
            action: HookAction::Tool {
                tool_id: purrcode_runtime_core::ToolId::native("command"),
                arguments: serde_json::json!({}),
            },
            blocking,
            timeout_seconds: 30,
            descriptor_digest: "abc".into(),
        }
    }

    #[test]
    fn a_denied_blocking_hook_aborts_and_is_audited() {
        let mut store = SessionStore::in_memory().unwrap();
        let session_id = SessionId::new();
        let hooks = vec![hook("h1", true)];
        let (outcomes, to_execute, aborted) = dispatch_hooks(
            &mut store,
            session_id,
            HookTrigger::BeforeWrite,
            &hooks,
            0,
            &|_| JudgmentDecision::Deny {
                reason: "policy refuses".into(),
            },
        );
        assert!(aborted, "a denied blocking hook aborts the chain");
        assert!(to_execute.is_empty(), "a denied hook must not execute");
        assert_eq!(outcomes.len(), 1);
        assert_eq!(outcomes[0].status, "denied");
        assert_eq!(outcomes[0].hook_id, "h1");
    }

    #[test]
    fn an_allowed_hook_is_scheduled_for_execution() {
        let mut store = SessionStore::in_memory().unwrap();
        let session_id = SessionId::new();
        let hooks = vec![hook("h1", false)];
        let (outcomes, to_execute, aborted) = dispatch_hooks(
            &mut store,
            session_id,
            HookTrigger::BeforeWrite,
            &hooks,
            0,
            &|_| {
                JudgmentDecision::AllowWithConstraints(ActionConstraints::read_only(PathBuf::from(
                    "/repo",
                )))
            },
        );
        assert!(!aborted);
        assert_eq!(to_execute.len(), 1, "an allowed hook must be scheduled");
        assert_eq!(to_execute[0].invocation.tool_id.as_str(), "native:command");
        assert_eq!(outcomes[0].status, "succeeded");
    }

    #[test]
    fn recursion_guard_skips_beyond_depth() {
        let mut store = SessionStore::in_memory().unwrap();
        let session_id = SessionId::new();
        let hooks = vec![hook("h1", false)];
        let (outcomes, _, _) = dispatch_hooks(
            &mut store,
            session_id,
            HookTrigger::AfterWrite,
            &hooks,
            5, // beyond the depth guard
            &|_| {
                JudgmentDecision::AllowWithConstraints(ActionConstraints::read_only(PathBuf::from(
                    "/repo",
                )))
            },
        );
        assert_eq!(outcomes[0].status, "skipped");
    }
}
