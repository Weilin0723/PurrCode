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

/// Build a hook's tool invocation, binding the REGISTERED TOOL's descriptor
/// digest.
///
/// This must not be the hook file's own digest. The executor recomputes
/// `digest_v3(action, constraints, descriptor_digest)` from the registry to
/// consume the authorization, so binding the hook's digest here produces an
/// authorization nothing can consume — every allowed hook would fail at
/// dispatch. Falling back to the hook digest when the tool is unregistered is
/// harmless: that hook is denied before it reaches execution.
fn hook_invocation(
    tool_id: &purrcode_runtime_core::ToolId,
    arguments: &serde_json::Value,
    constraints: &ActionConstraints,
    resolve_digest: &(dyn Fn(&purrcode_runtime_core::ToolId) -> Option<String> + Sync),
) -> ToolInvocation {
    ToolInvocation {
        tool_id: tool_id.clone(),
        arguments: arguments.clone(),
        working_directory: constraints.working_directory.clone(),
        descriptor_digest: resolve_digest(tool_id).unwrap_or_default(),
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
    resolve_digest: &(dyn Fn(&purrcode_runtime_core::ToolId) -> Option<String> + Sync),
) -> (
    JudgmentDecision,
    Option<ToolInvocation>,
    Option<ActionConstraints>,
) {
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
                    Some(hook_invocation(
                        tool_id,
                        arguments,
                        constraints,
                        resolve_digest,
                    )),
                    Some(constraints.clone()),
                );
            }
        }
        JudgmentDecision::RequireApproval { constraints, .. } => {
            // A hook that needs approval must produce a REAL approval boundary,
            // not just an audit row saying it wanted one. Propose the hook's
            // exact invocation and record the judgment, which is what moves the
            // session into `AwaitingApproval(action_id)` — the same durable
            // state a model-proposed action reaches.
            //
            // `POST /approve` then authorizes THIS action_id, binding the same
            // digest_v3 (action + constraints + descriptor digest), and the
            // executor's `consume_authorization` gives exactly-once: a replay
            // of the approval cannot run the hook twice.
            if let HookAction::Tool { tool_id, arguments } = &hook.action {
                let invocation = hook_invocation(tool_id, arguments, constraints, resolve_digest);
                let proposed = purrcode_runtime_core::ProposedAction::Tool(invocation.clone());
                let _ = store.append(
                    session_id,
                    &SessionEvent::ActionProposed {
                        action_id,
                        action: proposed,
                        turn_id: None,
                    },
                );
                let _ = store.append(
                    session_id,
                    &SessionEvent::JudgmentRecorded {
                        action_id,
                        decision: decision.clone(),
                        turn_id: None,
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
                    "awaiting_approval",
                    Some("approve this action to run the hook and continue the chain"),
                );
                return (
                    decision.clone(),
                    Some(invocation),
                    Some(constraints.clone()),
                );
            }
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
    resolve_digest: &(dyn Fn(&purrcode_runtime_core::ToolId) -> Option<String> + Sync),
) -> HookDispatch {
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
        return HookDispatch {
            outcomes,
            to_execute: Vec::new(),
            aborted: true,
            awaiting_approval: false,
        };
    }
    let mut outcomes = Vec::new();
    let mut to_execute = Vec::new();
    let mut aborted = false;
    let mut awaiting_approval = false;
    for hook in hooks {
        let (decision, invocation, constraints) =
            judge_hook(store, session_id, trigger, hook, evaluate, resolve_digest);
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
                    detail: Some(
                        "the session is awaiting approval for this hook; approving runs it \
                         exactly once and the chain continues"
                            .into(),
                    ),
                    action_id: None,
                });
                // A hook waiting on a human is a PAUSE, not a failure, so the
                // remaining hooks in the chain do not fire behind its back —
                // the same reason a blocking denial stops the chain. The
                // difference is that this one is resumable: the pending action
                // is durable, and approving it continues from here.
                awaiting_approval = true;
                break;
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
    HookDispatch {
        outcomes,
        to_execute,
        aborted,
        awaiting_approval,
    }
}

/// The result of dispatching one trigger.
///
/// `awaiting_approval` is deliberately distinct from `aborted`: an aborted
/// chain failed and the turn should fail with it, whereas an
/// approval-suspended chain is a durable pause with a pending action a person
/// can complete. Collapsing the two is what made "awaiting_approval" an audit
/// row with no way to act on it.
pub(crate) struct HookDispatch {
    pub outcomes: Vec<HookRunOutcome>,
    pub to_execute: Vec<HookExecution>,
    pub aborted: bool,
    pub awaiting_approval: bool,
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

    /// The registry descriptor digest a real dispatch would resolve.
    fn digests(_: &purrcode_runtime_core::ToolId) -> Option<String> {
        Some("registry-descriptor-digest".to_string())
    }

    fn session(store: &mut SessionStore) -> SessionId {
        let session_id = SessionId::new();
        store
            .append(
                session_id,
                &SessionEvent::SessionCreated {
                    objective: "hook lifecycle".into(),
                    repository: PathBuf::from("/repo"),
                    authority_mode: Default::default(),
                },
            )
            .unwrap();
        session_id
    }

    #[test]
    fn a_denied_blocking_hook_aborts_and_is_audited() {
        let mut store = SessionStore::in_memory().unwrap();
        let session_id = session(&mut store);
        let hooks = vec![hook("h1", true)];
        let dispatch = dispatch_hooks(
            &mut store,
            session_id,
            HookTrigger::BeforeWrite,
            &hooks,
            0,
            &|_| JudgmentDecision::Deny {
                reason: "policy refuses".into(),
            },
            &digests,
        );
        assert!(dispatch.aborted, "a denied blocking hook aborts the chain");
        assert!(
            dispatch.to_execute.is_empty(),
            "a denied hook must not execute"
        );
        assert_eq!(dispatch.outcomes.len(), 1);
        assert_eq!(dispatch.outcomes[0].status, "denied");
        assert_eq!(dispatch.outcomes[0].hook_id, "h1");
    }

    #[test]
    fn an_allowed_hook_is_scheduled_with_the_registry_descriptor_digest() {
        let mut store = SessionStore::in_memory().unwrap();
        let session_id = session(&mut store);
        let hooks = vec![hook("h1", false)];
        let dispatch = dispatch_hooks(
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
            &digests,
        );
        assert!(!dispatch.aborted);
        assert_eq!(
            dispatch.to_execute.len(),
            1,
            "an allowed hook must be scheduled"
        );
        assert_eq!(
            dispatch.to_execute[0].invocation.tool_id.as_str(),
            "native:command"
        );
        // The invocation must bind the TOOL's descriptor digest, not the hook
        // file's: the executor recomputes digest_v3 from the registry, so the
        // hook's own digest would produce an unconsumable authorization.
        assert_eq!(
            dispatch.to_execute[0].invocation.descriptor_digest,
            "registry-descriptor-digest"
        );
        assert_ne!(
            dispatch.to_execute[0].invocation.descriptor_digest,
            hooks[0].descriptor_digest
        );
        assert_eq!(dispatch.outcomes[0].status, "succeeded");
    }

    #[test]
    fn an_approval_requiring_hook_leaves_a_real_pending_approval() {
        // The closure the review asked for: `awaiting_approval` must be an
        // actionable boundary, not just an audit row. After dispatch the
        // session is AwaitingApproval on the hook's own action_id, and the
        // proposed action is the hook's exact invocation.
        let mut store = SessionStore::in_memory().unwrap();
        let session_id = session(&mut store);
        let hooks = vec![hook("h1", true)];
        let constraints = ActionConstraints::read_only(PathBuf::from("/repo"));
        let dispatch = dispatch_hooks(
            &mut store,
            session_id,
            HookTrigger::BeforeWrite,
            &hooks,
            0,
            &|_| JudgmentDecision::RequireApproval {
                reason: "destructive hook".into(),
                constraints: constraints.clone(),
            },
            &digests,
        );
        assert!(
            dispatch.awaiting_approval,
            "an approval-requiring hook suspends the chain"
        );
        assert!(
            !dispatch.aborted,
            "waiting for a human is a pause, not a failure"
        );
        assert!(
            dispatch.to_execute.is_empty(),
            "the hook must not run before the human approves"
        );

        let state = store.load(session_id).unwrap();
        let purrcode_runtime_core::SessionStatus::AwaitingApproval(pending) = state.status else {
            panic!(
                "the session must be awaiting approval, got {:?}",
                state.status
            );
        };
        let action = state
            .proposed_actions
            .get(&pending)
            .expect("the pending action is durable");
        let purrcode_runtime_core::ProposedAction::Tool(invocation) = action else {
            panic!("a hook's pending action is its tool invocation");
        };
        assert_eq!(invocation.tool_id.as_str(), "native:command");
        assert_eq!(invocation.descriptor_digest, "registry-descriptor-digest");
        assert!(matches!(
            state.judgments.get(&pending),
            Some(JudgmentDecision::RequireApproval { .. })
        ));

        // Exactly-once: authorizing the pending action binds digest_v3, and the
        // authorization can be consumed only once — a replayed approval cannot
        // run the hook a second time.
        let proposed = purrcode_runtime_core::ProposedAction::Tool(invocation.clone());
        let digest = proposed
            .digest_v3(&constraints, &invocation.descriptor_digest)
            .unwrap();
        store
            .authorize(&purrcode_runtime_core::Authorization {
                action_id: pending,
                session_id,
                action_digest: digest.clone(),
                constraints: constraints.clone(),
                authorized_at: chrono::Utc::now(),
                approved_by: purrcode_runtime_core::ApprovalAuthority::Human,
            })
            .unwrap();
        assert!(
            store.consume_authorization(pending, &digest).is_ok(),
            "the approved hook runs once"
        );
        assert!(
            store.consume_authorization(pending, &digest).is_err(),
            "and cannot run a second time"
        );
    }

    #[test]
    fn an_approval_requiring_hook_stops_later_hooks_from_firing_behind_it() {
        let mut store = SessionStore::in_memory().unwrap();
        let session_id = session(&mut store);
        let hooks = vec![hook("h1", false), hook("h2", false)];
        let dispatch = dispatch_hooks(
            &mut store,
            session_id,
            HookTrigger::BeforeWrite,
            &hooks,
            0,
            &|hook| {
                if hook.id == "h1" {
                    JudgmentDecision::RequireApproval {
                        reason: "needs a human".into(),
                        constraints: ActionConstraints::read_only(PathBuf::from("/repo")),
                    }
                } else {
                    JudgmentDecision::AllowWithConstraints(ActionConstraints::read_only(
                        PathBuf::from("/repo"),
                    ))
                }
            },
            &digests,
        );
        assert!(dispatch.awaiting_approval);
        assert_eq!(dispatch.outcomes.len(), 1, "the chain stopped at h1");
        assert!(dispatch.to_execute.is_empty());
    }

    #[test]
    fn recursion_guard_skips_beyond_depth() {
        let mut store = SessionStore::in_memory().unwrap();
        let session_id = session(&mut store);
        let hooks = vec![hook("h1", false)];
        let dispatch = dispatch_hooks(
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
            &digests,
        );
        assert_eq!(dispatch.outcomes[0].status, "skipped");
    }
}
