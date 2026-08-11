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
    ActionConstraints, ActionId, HookDescriptor, HookTrigger, JudgmentDecision, SessionEvent,
    SessionId, ToolInvocation,
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

/// What a `HookAction` resolves to before anything judges it.
///
/// Resolution happens ONCE, up front, and everything downstream sees a concrete
/// invocation. The bug this closes: `HookAction::Capability` was resolved
/// inside the PawGate closure but the execution path still pattern-matched on
/// `HookAction::Tool`, so a capability hook was judged, allowed, recorded as
/// "succeeded" — and never ran. A false success in an audit trail is worse than
/// a failure.
pub(crate) type ResolvedHookAction = Result<ToolInvocation, String>;

/// A single judgment of one hook, over its already-resolved invocation.
pub(crate) struct JudgedHook {
    pub decision: JudgmentDecision,
    pub invocation: Option<ToolInvocation>,
    pub constraints: Option<ActionConstraints>,
    /// The pending action a human approves, when the decision was
    /// `RequireApproval`.
    pub pending_action_id: Option<ActionId>,
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
    resolve: &(dyn Fn(&HookDescriptor) -> ResolvedHookAction + Sync),
    evaluate: &(dyn Fn(&HookDescriptor, &ToolInvocation) -> JudgmentDecision + Sync),
) -> JudgedHook {
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
    // Resolve first. A hook whose action cannot become a concrete invocation is
    // DENIED here — it can never reach a state where it is judged allowed but
    // has nothing to execute.
    let invocation = match resolve(hook) {
        Ok(invocation) => invocation,
        Err(reason) => {
            let _ = record_hook_run(
                store,
                session_id,
                &hook.id,
                &hook.descriptor_digest,
                trigger,
                &hook.layer,
                Some(action_id),
                "denied",
                Some(&reason),
            );
            return JudgedHook {
                decision: JudgmentDecision::Deny { reason },
                invocation: None,
                constraints: None,
                pending_action_id: None,
            };
        }
    };
    let decision = evaluate(hook, &invocation);
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
            return JudgedHook {
                decision: decision.clone(),
                invocation: Some(with_working_directory(invocation, constraints)),
                constraints: Some(constraints.clone()),
                pending_action_id: None,
            };
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
            let invocation = with_working_directory(invocation, constraints);
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
            return JudgedHook {
                decision: decision.clone(),
                invocation: Some(invocation),
                constraints: Some(constraints.clone()),
                pending_action_id: Some(action_id),
            };
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
    JudgedHook {
        decision,
        invocation: None,
        constraints: None,
        pending_action_id: None,
    }
}

/// Re-home an invocation onto the directory PawGate actually authorized.
fn with_working_directory(
    mut invocation: ToolInvocation,
    constraints: &ActionConstraints,
) -> ToolInvocation {
    invocation.working_directory = constraints.working_directory.clone();
    invocation
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
    completed: &std::collections::BTreeSet<String>,
    resolve: &(dyn Fn(&HookDescriptor) -> ResolvedHookAction + Sync),
    evaluate: &(dyn Fn(&HookDescriptor, &ToolInvocation) -> JudgmentDecision + Sync),
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
            suspension: None,
        };
    }
    let mut outcomes = Vec::new();
    let mut to_execute = Vec::new();
    let mut aborted = false;
    let mut suspension = None;
    // Hooks satisfied on an earlier pass of this same action. They already ran
    // (or were already approved), so firing them again would re-ask for an
    // approval the user just granted and the action would never proceed.
    let mut ran: Vec<String> = Vec::new();
    for hook in hooks {
        if completed.contains(&hook.id) {
            outcomes.push(HookRunOutcome {
                hook_id: hook.id.clone(),
                status: "already_satisfied",
                detail: Some("this hook already ran for the deferred action".into()),
                action_id: None,
            });
            ran.push(hook.id.clone());
            continue;
        }
        let judged = judge_hook(store, session_id, trigger, hook, resolve, evaluate);
        match judged.decision {
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
                if let (Some(invocation), Some(constraints)) =
                    (judged.invocation, judged.constraints)
                {
                    to_execute.push(HookExecution {
                        hook_id: hook.id.clone(),
                        hook_digest: hook.descriptor_digest.clone(),
                        layer: hook.layer,
                        invocation,
                        constraints,
                    });
                    ran.push(hook.id.clone());
                    outcomes.push(HookRunOutcome {
                        hook_id: hook.id.clone(),
                        status: "succeeded",
                        detail: None,
                        action_id: None,
                    });
                } else {
                    // Allowed with nothing to run is the false-success case.
                    // It is a failure, and a blocking hook in that state stops
                    // the chain like any other blocking failure.
                    outcomes.push(HookRunOutcome {
                        hook_id: hook.id.clone(),
                        status: "failed",
                        detail: Some(
                            "the hook was allowed but produced no executable invocation".into(),
                        ),
                        action_id: None,
                    });
                    if hook.blocking {
                        aborted = true;
                        break;
                    }
                }
            }
            JudgmentDecision::RequireApproval { reason, .. } => {
                outcomes.push(HookRunOutcome {
                    hook_id: hook.id.clone(),
                    status: "awaiting_approval",
                    detail: Some(
                        "the session is awaiting approval for this hook; approving runs it \
                         exactly once and the chain continues"
                            .into(),
                    ),
                    action_id: judged.pending_action_id,
                });
                // A hook waiting on a human is a PAUSE, not a failure, so the
                // remaining hooks in the chain do not fire behind its back —
                // the same reason a blocking denial stops the chain. The
                // difference is that this one is resumable: the pending action
                // is durable, and approving it continues from here.
                if let Some(pending) = judged.pending_action_id {
                    // The suspended hook counts as satisfied on resume: the
                    // approval runs it, so re-firing it would loop forever.
                    ran.push(hook.id.clone());
                    suspension = Some(HookSuspensionRecord {
                        hook_id: hook.id.clone(),
                        hook_action_id: pending,
                        reason,
                        completed_hooks: ran.clone(),
                    });
                    break;
                }
                // No durable pending action means there is nothing to approve.
                // Refusing to continue is the honest outcome.
                aborted = true;
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
        suspension,
    }
}

/// The chain stopped on a hook that needs a person.
#[derive(Clone, Debug)]
pub(crate) struct HookSuspensionRecord {
    pub hook_id: String,
    pub hook_action_id: ActionId,
    pub reason: String,
    pub completed_hooks: Vec<String>,
}

/// The result of dispatching one trigger.
///
/// `suspension` is deliberately distinct from `aborted`: an aborted chain
/// failed and the turn should fail with it, whereas an approval-suspended chain
/// is a durable pause with a pending action a person can complete. Collapsing
/// the two is what made "awaiting_approval" an audit row with no way to act on
/// it — and then a `SessionFailed`.
pub(crate) struct HookDispatch {
    pub outcomes: Vec<HookRunOutcome>,
    pub to_execute: Vec<HookExecution>,
    pub aborted: bool,
    pub suspension: Option<HookSuspensionRecord>,
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
    use purrcode_runtime_core::HookAction;
    use std::collections::BTreeSet;
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

    /// A capability hook: names an intent, not a tool. This is the shape that
    /// used to be judged and then silently skipped at execution.
    fn capability_hook(id: &str, blocking: bool) -> HookDescriptor {
        HookDescriptor {
            action: HookAction::Capability {
                id: purrcode_runtime_core::CapabilityId::parse("security_scan").unwrap(),
            },
            blocking,
            ..hook(id, blocking)
        }
    }

    /// The daemon's real resolver, in miniature: a `Tool` action keeps its own
    /// id, a `Capability` action resolves to one, and both bind the REGISTRY's
    /// descriptor digest rather than the hook file's.
    fn resolve(hook: &HookDescriptor) -> ResolvedHookAction {
        let tool_id = match &hook.action {
            HookAction::Tool { tool_id, .. } => tool_id.clone(),
            HookAction::Capability { .. } => purrcode_runtime_core::ToolId::native("command"),
        };
        Ok(ToolInvocation {
            tool_id,
            arguments: serde_json::json!({}),
            working_directory: PathBuf::from("/repo"),
            descriptor_digest: "registry-descriptor-digest".into(),
        })
    }

    fn allow(_: &HookDescriptor, _: &ToolInvocation) -> JudgmentDecision {
        JudgmentDecision::AllowWithConstraints(ActionConstraints::read_only(PathBuf::from("/repo")))
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
            &BTreeSet::new(),
            &resolve,
            &|_, _| JudgmentDecision::Deny {
                reason: "policy refuses".into(),
            },
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
            &BTreeSet::new(),
            &resolve,
            &allow,
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
    fn an_allowed_capability_hook_actually_executes() {
        // The regression: a capability hook resolved to a tool, was judged
        // Allow, and was recorded "succeeded" — but the execution path matched
        // only `HookAction::Tool`, so nothing ran. "Succeeded" for a hook that
        // never fired is a false entry in the audit trail, and the security
        // scanner nobody noticed was not running is exactly the case that
        // matters.
        let mut store = SessionStore::in_memory().unwrap();
        let session_id = session(&mut store);
        let hooks = vec![capability_hook("scan", false)];
        let dispatch = dispatch_hooks(
            &mut store,
            session_id,
            HookTrigger::BeforeWrite,
            &hooks,
            0,
            &BTreeSet::new(),
            &resolve,
            &allow,
        );
        assert_eq!(
            dispatch.to_execute.len(),
            1,
            "a capability hook that PawGate allowed must be scheduled to run"
        );
        assert_eq!(
            dispatch.to_execute[0].invocation.tool_id.as_str(),
            "native:command",
            "the scheduled invocation is the resolved provider"
        );
        assert_eq!(dispatch.outcomes[0].status, "succeeded");
    }

    #[test]
    fn an_unresolvable_hook_is_denied_not_silently_successful() {
        let mut store = SessionStore::in_memory().unwrap();
        let session_id = session(&mut store);
        let hooks = vec![capability_hook("scan", true)];
        let dispatch = dispatch_hooks(
            &mut store,
            session_id,
            HookTrigger::BeforeWrite,
            &hooks,
            0,
            &BTreeSet::new(),
            &|hook| Err(format!("no provider satisfies `{}`", hook.id)),
            &allow,
        );
        assert!(dispatch.to_execute.is_empty());
        assert_eq!(dispatch.outcomes[0].status, "denied");
        assert!(dispatch.aborted, "a blocking hook that cannot run aborts");
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
            &BTreeSet::new(),
            &resolve,
            &|_, _| JudgmentDecision::RequireApproval {
                reason: "destructive hook".into(),
                constraints: ActionConstraints::read_only(PathBuf::from("/repo")),
            },
        );
        let suspension = dispatch
            .suspension
            .as_ref()
            .expect("an approval-requiring hook suspends the chain");
        assert!(
            !dispatch.aborted,
            "waiting for a human is a pause, not a failure"
        );
        assert!(
            dispatch.to_execute.is_empty(),
            "the hook must not run before the human approves"
        );
        assert_eq!(
            suspension.completed_hooks,
            vec!["h1".to_string()],
            "the suspended hook counts as satisfied on resume, or approving it would ask again"
        );

        let state = store.load(session_id).unwrap();
        let purrcode_runtime_core::SessionStatus::AwaitingApproval(pending) = state.status else {
            panic!(
                "the session must be awaiting approval, got {:?}",
                state.status
            );
        };
        assert_eq!(
            suspension.hook_action_id, pending,
            "the suspension names the action the human will approve"
        );
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
            &BTreeSet::new(),
            &resolve,
            &|hook, _| {
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
        );
        assert!(dispatch.suspension.is_some());
        assert_eq!(dispatch.outcomes.len(), 1, "the chain stopped at h1");
        assert!(dispatch.to_execute.is_empty());
    }

    #[test]
    fn resuming_skips_the_hook_that_was_already_approved_and_runs_the_rest() {
        // What makes the suspension converge. On resume the approved hook must
        // not fire again — it already ran through the approval — while the rest
        // of the chain, which never fired, still has to.
        let mut store = SessionStore::in_memory().unwrap();
        let session_id = session(&mut store);
        let hooks = vec![hook("h1", false), hook("h2", false)];
        let completed = BTreeSet::from(["h1".to_string()]);
        let dispatch = dispatch_hooks(
            &mut store,
            session_id,
            HookTrigger::BeforeWrite,
            &hooks,
            0,
            &completed,
            &resolve,
            &|hook, _| {
                if hook.id == "h1" {
                    // Would suspend again — and loop forever — if it fired.
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
        );
        assert!(
            dispatch.suspension.is_none(),
            "the already-approved hook must not ask for approval a second time"
        );
        assert_eq!(dispatch.outcomes[0].status, "already_satisfied");
        assert_eq!(
            dispatch.to_execute.len(),
            1,
            "the rest of the chain still runs"
        );
        assert_eq!(dispatch.to_execute[0].hook_id, "h2");
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
            &BTreeSet::new(),
            &resolve,
            &allow,
        );
        assert_eq!(dispatch.outcomes[0].status, "skipped");
    }
}
