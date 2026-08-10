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
#[allow(dead_code)]
pub(crate) struct HookRunOutcome {
    pub hook_id: String,
    pub status: &'static str,
    pub detail: Option<String>,
    pub action_id: Option<ActionId>,
}

/// Dispatch one trigger against a set of hooks. Returns the per-hook outcomes.
///
/// Hooks run sequentially. A `blocking` hook that fails aborts the remaining
/// hooks and reports `failed`; a non-blocking hook failure is recorded and the
/// chain continues. Hook recursion is bounded by the depth guard — an
/// `after_write` hook that itself writes must not fire itself.
///
/// This is the daemon's governed-hook surface: the lifecycle trigger points
/// (before_write / after_write / after_agent_complete / before_commit /
/// after_validation) call it with the hook set for the session's repository
/// and closures that route through PawGate + the store. The dispatcher itself
/// is unit-tested here; the trigger wiring lands with the integration that
/// threads the extension set into the turn.
#[allow(dead_code)]
pub(crate) async fn dispatch_hooks(
    store: &mut SessionStore,
    session_id: SessionId,
    trigger: HookTrigger,
    hooks: &[HookDescriptor],
    depth: u8,
    // A context in which a hook can authorize itself. In practice the daemon
    // supplies the policy/registry handle; the hook action is a registered
    // ToolInvocation that flows through the same PawGate path.
    evaluate: &dyn Fn(&HookDescriptor) -> JudgmentDecision,
    execute: &dyn Fn(ActionId, &ToolInvocation, &ActionConstraints),
) -> Vec<HookRunOutcome> {
    if depth > 4 {
        // A hook that recurses past the guard is cut off; the session is never
        // wedged by its own hooks.
        return hooks
            .iter()
            .map(|hook| HookRunOutcome {
                hook_id: hook.id.clone(),
                status: "skipped",
                detail: Some("hook recursion depth exceeded".into()),
                action_id: None,
            })
            .collect();
    }
    let mut outcomes = Vec::new();
    for hook in hooks {
        let action_id = ActionId::new();
        // Record BEFORE proposing so a denied hook is auditable.
        let _ = store.append(
            session_id,
            &SessionEvent::HookTriggered {
                hook_id: hook.id.clone(),
                trigger,
                action_id: Some(action_id),
            },
        );
        let decision = evaluate(hook);
        match decision {
            JudgmentDecision::Deny { reason } => {
                outcomes.push(HookRunOutcome {
                    hook_id: hook.id.clone(),
                    status: "denied",
                    detail: Some(reason),
                    action_id: Some(action_id),
                });
                if hook.blocking {
                    return outcomes;
                }
            }
            JudgmentDecision::AllowWithConstraints(constraints) => {
                if let HookAction::Tool { tool_id, arguments } = &hook.action {
                    execute(
                        action_id,
                        &ToolInvocation {
                            tool_id: tool_id.clone(),
                            arguments: arguments.clone(),
                            working_directory: constraints.working_directory.clone(),
                            descriptor_digest: hook.descriptor_digest.clone(),
                        },
                        &constraints,
                    );
                }
                outcomes.push(HookRunOutcome {
                    hook_id: hook.id.clone(),
                    status: "succeeded",
                    detail: None,
                    action_id: Some(action_id),
                });
            }
            JudgmentDecision::RequireApproval { .. } => {
                outcomes.push(HookRunOutcome {
                    hook_id: hook.id.clone(),
                    status: "awaiting_approval",
                    detail: None,
                    action_id: Some(action_id),
                });
                if hook.blocking {
                    return outcomes;
                }
            }
            _ => {
                outcomes.push(HookRunOutcome {
                    hook_id: hook.id.clone(),
                    status: "failed",
                    detail: Some("unsupported hook decision".into()),
                    action_id: Some(action_id),
                });
                if hook.blocking {
                    return outcomes;
                }
            }
        }
    }
    outcomes
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

    #[tokio::test]
    async fn a_denied_blocking_hook_aborts_and_is_audited() {
        let mut store = SessionStore::in_memory().unwrap();
        let session_id = SessionId::new();
        let hooks = vec![hook("h1", true)];
        let outcomes = dispatch_hooks(
            &mut store,
            session_id,
            HookTrigger::BeforeWrite,
            &hooks,
            0,
            &|_| JudgmentDecision::Deny {
                reason: "policy refuses".into(),
            },
            &|_action_id, _invocation, _constraints| {},
        )
        .await;
        assert_eq!(outcomes.len(), 1);
        assert_eq!(outcomes[0].status, "denied");
        assert_eq!(outcomes[0].hook_id, "h1");
    }

    #[tokio::test]
    async fn an_allowed_hook_executes() {
        let mut store = SessionStore::in_memory().unwrap();
        let session_id = SessionId::new();
        let hooks = vec![hook("h1", false)];
        let executed = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let executed_for_closure = executed.clone();
        let _ = dispatch_hooks(
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
            &|_action_id, _invocation, _constraints| {
                executed_for_closure.store(true, std::sync::atomic::Ordering::SeqCst);
            },
        )
        .await;
        assert!(
            executed.load(std::sync::atomic::Ordering::SeqCst),
            "an allowed hook's action must execute"
        );
    }

    #[tokio::test]
    async fn recursion_guard_skips_beyond_depth() {
        let mut store = SessionStore::in_memory().unwrap();
        let session_id = SessionId::new();
        let hooks = vec![hook("h1", false)];
        let outcomes = dispatch_hooks(
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
            &|_action_id, _invocation, _constraints| {},
        )
        .await;
        assert_eq!(outcomes[0].status, "skipped");
    }
}
