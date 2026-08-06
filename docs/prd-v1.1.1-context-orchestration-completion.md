# PurrCode v1.1.1 — Context Orchestration Completion & Agent UX

**Status:** PRD (Release Blocker)
**Target:** `feature/v1.1-context-orchestration` → `main`
**Branches from:** `feature/v1.1-context-orchestration` (commit `3994601`)
**Author:** Architecture Review — all 30 items confirmed via code inspection against v1.1 PRD acceptance criteria

---

## Executive Summary

v1.1 built the primitives right: Context Ledger, Semantic Checkpoint, Whisker v2, multi-read actions[], Scout scaffolding, and TurnId correlation. But several critical loops are not closed. This PRD does **not** rewrite v1.1. It completes it.

**Rating:** 5.5/10 for "Context Orchestration done" → target **8.5/10** after this PRD.

The highest-priority fix is the stale-ledger compaction loop (P0-1): the agent can burn all 32 autonomous iterations re-compacting against the same 80k ledger entry because compaction never reassembles context afterward. Every other P0 is similarly a closed-loop defect — a feature whose data flows look correct in isolation but whose control flow does not re-enter the correct path.

---

## Priority Tiers

| Tier | Count | Scope |
|------|-------|-------|
| **P0** — release blocker | 10 items | Compaction correctness, batch-auth safety, Scout actually working, TurnId lifecycle, ledger fidelity |
| **P1** — product feel | 14 items | Unified model schema, semantic checkpoint population, token-based retention, model-aware limits, Context UI, manual compact |
| **P2** — next cycle | 6 items | Reranker, embedding, parallel Scouts, cache awareness |

---

---

# P0-1: Fix Stale-Ledger Compaction Loop

## Current Defect

`run_until_pause()` (agent.rs:1677-1689) reads `state.recent_context_ledger.back()` to decide whether to compact. After a `CheckpointCompacted` event is appended and the loop `continue`s, the **next iteration re-reads the same ledger entry** — because compaction never calls `build_messages()` or generates a new `ContextAssembled` event. The loop spins:

```
Turn N   → ContextAssembled (ledger=80k, above 70% threshold)
Turn N+1 → reads back ledger=80k → COMPACT → continue
Turn N+2 → reads back ledger=80k → COMPACT → continue
...repeats until MAX_AUTONOMOUS_ITERATIONS (32)
```

The token guard (agent.rs:1677-1689) checks `token_pressure` from the most recent `ContextLedgerEntry`, which was appended by the **previous** normal iteration — not by the compaction path. Since compaction appends `CheckpointCompacted` but not `ContextAssembled`, `recent_context_ledger.back()` keeps returning the pre-compaction entry.

## Target Behavior

**Preflight model:** assemble → estimate → decide → compact only if needed → reassemble → send.

```
run_until_pause() iteration:
  1. assemble current request  (build_messages → ContextAssembled)
  2. estimate current context   (provider.count_tokens on actual ModelRequest)
  3. does it fit?
     ├── yes → send to model
     └── no  → compact ONCE
                → reassemble
                → estimate again
                ├── fits → send
                └── still doesn't fit → fail with ContextOverflow (not retry)
```

Maximum **one automatic compaction per turn**. If the context still overflows after one compaction, fail closed with a clear error.

## Data Contract Changes

### New `AgentError` variant
```rust
AgentError::ContextOverflow {
    estimated_tokens: u64,
    max_input_tokens: u64,
    after_compaction: bool,
}
```

### Modified compaction trigger
Remove `token_pressure` check from iteration preamble. Instead, move it to a `prepare_context()` function:

```rust
async fn prepare_context(&self, ...) -> Result<(Vec<ModelMessage>, ContextLedgerEntry, bool), AgentError> {
    let (messages, ledger) = build_messages(...);
    let estimate = provider.count_tokens(&ModelRequest { messages: messages.clone(), .. }).await?;
    if estimate.tokens <= effective_input_capacity() {
        return Ok((messages, ledger, false)); // no compaction needed
    }
    // Compact once, then rebuild
    compact(state)?;
    let (messages2, ledger2) = build_messages(...);
    let estimate2 = provider.count_tokens(...).await?;
    if estimate2.tokens <= effective_input_capacity() {
        return Ok((messages2, ledger2, true));
    }
    Err(AgentError::ContextOverflow { ... })
}
```

### `effective_input_capacity()`
```rust
fn effective_input_capacity(&self) -> u64 {
    let model_limit = self.provider_for("coding_worker").context_limit();
    let budget_limit = self.budget().maximum_input_tokens.unwrap_or(u64::MAX);
    let reserved_output = 8192; // or self.budget().maximum_output_tokens
    model_limit
        .saturating_sub(reserved_output)
        .min(budget_limit)
}
```

## Implementation Files

| File | Change |
|------|--------|
| `crates/agent-runtime/src/agent.rs` | Move compaction trigger from iteration preamble (`:1652-1833`) to new `prepare_context()` called after `build_messages()` / before `prepare_model_request()` |
| `crates/agent-runtime/src/context.rs` | No structural change; `build_messages()` already returns `(messages, ledger)` |
| `crates/agent-runtime/src/errors.rs` | Add `ContextOverflow` variant |
| `crates/provider-gateway/src/lib.rs` | Expose `context_limit()` on `ModelProvider` trait (or read from `ModelId` capabilities) |

## Tests

1. **compaction_does_not_loop_on_stale_ledger**: Create a session whose first turn produces a ledger above the compaction threshold. Assert that exactly one `CheckpointCompacted` event is emitted, and the second iteration proceeds to a model call rather than compacting again.
2. **context_overflow_after_compaction_fails**: Create a session with a tiny input budget (e.g., 512 tokens). Assert that after one compaction, the agent returns `ContextOverflow` rather than spinning.
3. **compaction_rebuilds_ledger**: After compaction, verify that `recent_context_ledger.back()` shows an entry with `turn_id` matching the new iteration (not the pre-compaction turn).

## Acceptance Criteria

- [ ] Agent never compacts more than once per `run_until_pause()` iteration
- [ ] After compaction, `recent_context_ledger.back()` reflects the rebuilt context, not the pre-compaction state
- [ ] Context overflow after compaction fails with `ContextOverflow`, not `IterationLimit`
- [ ] The compaction guard in iteration preamble (`:1677-1689`) is removed or restructured as a preflight check

---

# P0-2: Compaction Must Be Current-Request Preflight

## Current Defect

Compaction fires at the *start* of an iteration based on the *previous* iteration's ledger. This is a stale read: the context being compacted may not be the context about to be sent.

## Target Behavior

Context estimation and compaction happen **after** `build_messages()` and **before** the model call — inside the same iteration, on the actual `ModelRequest` about to be sent.

## Implementation

This is the same change as P0-1. The preflight loop is the fix for both.

## Acceptance Criteria

- [ ] `provider.count_tokens()` is called on the actual `ModelRequest` before deciding to compact
- [ ] The compaction decision uses the current request's token count, not a historical ledger entry

---

# P0-3: Ledger Must Reflect FINAL ModelRequest

## Current Defect A: Missing post-ledger message injection

`build_messages()` (context.rs:607-993) returns `(messages, ContextLedgerEntry)`, and the ledger is recorded via `ContextAssembled`. Then `agent.rs:1900-1924` injects the "EFFECTIVE DAEMON CONTRACT" system message, and `agent.rs:1926-1946` may inject a "STEP LIMIT WARNING" message — neither of which appears in the ledger.

The model actually sees these messages, but the inspector does not.

## Current Defect B: Token estimator mismatch

The ledger uses `chars().count().div_ceil(4)` (context.rs:970-973), while `prepare_model_request()` (agent.rs:319) calls `provider.count_tokens(&request)`. These are not the same estimator for any tokenizer that is not character-based. The code comment claims "structurally guaranteed to equal aggregate estimate" but this is only true when the default `ProviderRouter::count_tokens` (provider-gateway/src/lib.rs:2069-2079) — which also uses `div_ceil(4)` — is called. When a real provider's tokenizer runs, the two diverge.

## Target Behavior

### ContextEnvelope pattern

Instead of `build_messages()` returning a ledger it computed halfway through assembly, introduce:

```rust
struct ContextEnvelope {
    instructions: Vec<ModelMessage>,
    controls: Vec<ModelMessage>,       // daemon contract, step limit
    conversation: Vec<ModelMessage>,
    task_state: String,
    checkpoint: String,
    retrieved: String,
    output_contract: String,
    reserve: u64,
}

impl ContextEnvelope {
    fn render(&self) -> Vec<ModelMessage> { ... }
    fn ledger(&self) -> ContextLedgerEntry { ... }
}
```

The ledger is computed from the **final rendered messages** — not from intermediate string pieces.

### Token source alignment

The ledger's `total_estimated_tokens` should come from `provider.count_tokens()` on the final `ModelRequest`, not from the `div_ceil(4)` heuristic. When the provider cannot count tokens (e.g., Ollama without a tokenizer endpoint), fall back to `div_ceil(4)` and mark the entry with `estimator: "heuristic"`.

Add to `ContextLedgerEntry`:
```rust
pub estimator: TokenEstimator,
// enum TokenEstimator { ProviderCounted, CharDiv4 }
```

## Implementation Files

| File | Change |
|------|--------|
| `crates/runtime-core/src/lib.rs` | Add `TokenEstimator` enum, `ContextEnvelope` type (or keep in agent-runtime), add `estimator` field to `ContextLedgerEntry` |
| `crates/agent-runtime/src/context.rs` | Refactor `build_messages()` into `ContextEnvelope::assemble()` + `ContextEnvelope::render()` + `ContextEnvelope::ledger()` |
| `crates/agent-runtime/src/agent.rs` | Move daemon contract + step limit injection into `ContextEnvelope::render()`, record `ContextAssembled` AFTER final render |

## Tests

1. **ledger_includes_daemon_contract**: After `ContextEnvelope::render()`, verify the ledger accounts for the daemon contract message.
2. **ledger_includes_step_limit_warning**: At MAX_AUTONOMOUS_ITERATIONS-1, verify the step limit warning appears in the ledger.
3. **provider_counted_ledger_matches_request**: When `provider.count_tokens()` succeeds, verify `ledger.total_estimated_tokens == count_tokens_result.tokens`.

## Acceptance Criteria

- [ ] Daemon contract message is accounted for in the context ledger
- [ ] Step limit warning is accounted for in the context ledger
- [ ] Ledger `total_estimated_tokens` comes from `provider.count_tokens()` when available
- [ ] `estimator` field distinguishes provider-counted from heuristic
- [ ] Inspector shows what the model actually saw, not a partial projection

---

# P0-4: Batch Read Must Use PawGate's Per-Action Constraints

## Current Defect

agent.rs:2103-2181 — the multi-read batch path:
1. Normalizes each action
2. Proposes each action through PawGate (`policy.evaluate()`)
3. Records each `JudgmentDecision`
4. Then **discards** the per-action constraints and creates a shared `ActionConstraints::read_only(worktree.clone())` (hardcoded: timeout=120s, max_output=1MB)

This means:
- If PawGate returns `AllowWithConstraints(timeout=30s, max_output=100KB)`, the batch executes with `timeout=120s, max_output=1MB`
- The `action.digest(&constraints)` uses the hardcoded constraints, not PawGate's — so the authorization digest doesn't match what PawGate approved
- The single-action path (agent.rs:2656-2665) correctly uses PawGate's constraints. Only the batch path is broken.

## Target Behavior

Each action in a batch carries its own PawGate-returned constraints:

```rust
struct AuthorizedRead {
    action_id: ActionId,
    action: ProposedAction,
    constraints: ActionConstraints,  // from PawGate, not hardcoded
}

// Batch execution:
let mut authorized: Vec<AuthorizedRead> = Vec::new();
for action in &normalized {
    let decision = self.policy.evaluate(action, &worktree);
    let constraints = decision_constraints(&decision)
        .ok_or(...)?
        .clone();
    authorized.push(AuthorizedRead { action_id, action, constraints });
}
// Each AuthorizedRead uses its own constraints for digest + execution
```

## Implementation Files

| File | Change |
|------|--------|
| `crates/agent-runtime/src/agent.rs` | Lines 2097-2181: store per-action constraints from PawGate, use them for digest + authorization + execution |
| `crates/claw-sandbox/src/lib.rs` | Verify `execute_batch` already accepts per-action constraints or add overload |

## Tests

1. **batch_respects_per_action_constraints**: PawGate policy with `timeout_seconds=30`. Verify batch execution uses 30s timeout, not 120s.
2. **batch_digest_matches_pawgate_approval**: Verify `action.digest(&pawgate_constraints)` matches the authorization record.
3. **batch_fails_individual_denied_action**: If one action in a batch is denied, only that action fails; others proceed.

## Acceptance Criteria

- [ ] Batch path stores PawGate-returned constraints per action
- [ ] `Authorization` digest uses PawGate constraints, not hardcoded `read_only()`
- [ ] `execute_batch` receives per-action constraints
- [ ] The `ActionConstraints::read_only()` hardcode at agent.rs:2141 is removed from the batch path

---

# P0-5: Scout Tool Outputs Must Feed Back Into Scout Model Turns

## Current Defect

`run_scout()` (agent.rs:1209-1362) builds initial messages (line 1230-1234), then enters a loop (line 1242-1346). The loop:
1. Creates `ModelRequest { messages: messages.clone(), ... }` (line 1251-1257)
2. Gets a turn from the model (line 1259-1269)
3. Executes the action (line 1320-1329)
4. **Never appends the action result to `messages`**

The model never sees the results of its own reads. On the next iteration, `messages.clone()` returns the same initial prompt. This is repeated fresh inference, not an exploration loop.

## Target Behavior

After executing a scout action, append the result to `messages`:

```rust
// After execution:
messages.push(ModelMessage {
    role: "user".into(),
    content: format!(
        "Read result for {}:\n{}",
        action_description,
        bounded_terminal_text(&execution.stdout)
    ),
});
```

Also append the model's turn as an assistant message so the conversation history is coherent.

## Implementation Files

| File | Change |
|------|--------|
| `crates/agent-runtime/src/agent.rs` | `run_scout()` loop: append action results to `messages` after each execution |

## Tests

1. **scout_sees_read_results**: Mock a scout session with 2 read actions. Verify messages[2] contains the first action, messages[3] contains its result, messages[4] contains the second action.

## Acceptance Criteria

- [ ] Scout's `messages` vector grows with each turn's action + result
- [ ] The model sees the output of its previous reads before deciding the next action
- [ ] Scout produces evidence that reflects sequential exploration, not repeated first reads

---

# P0-6: Scout Must Support Multi-Read (actions[])

## Current Defect

`run_scout()` line 1279-1283:
```rust
let action = turn
    .action
    .clone()
    .ok_or_else(|| AgentError::InvalidModelTurn("scout action is required".into()))?;
```

Scout only reads `turn.action`. If the model returns `actions[]` (the multi-read array), Scout ignores it.

## Target Behavior

Scout should process `turn.actions` when non-empty, or fall back to `turn.action`:

```rust
let actions: Vec<AgentAction> = if !turn.actions.is_empty() {
    turn.actions.clone()
} else {
    vec![turn.action.clone()
        .ok_or_else(|| AgentError::InvalidModelTurn("scout action is required".into()))?]
};
```

All actions must be read-only. Process them as a batch through PawGate + Claw, same as the main agent loop.

## Implementation Files

| File | Change |
|------|--------|
| `crates/agent-runtime/src/agent.rs` | `run_scout()`: support `turn.actions` array |
| `crates/agent-runtime/src/context.rs` | `build_scout_messages()`: update prompt to inform model it can use multi-read |

## Acceptance Criteria

- [ ] Scout processes `actions[]` when present
- [ ] Scout falls back to `action` for backward compatibility
- [ ] Scout prompt mentions multi-read capability

---

# P0-7: Scout Must Be Integrated Into Agent Router

## Current Defect

`run_scout()` exists as a public method on `NativeAgent`, but `run_until_pause()` never calls it. There is no `ScoutRequest` dispatch, no agent router, and no decision point where the main agent says "this is a complex exploration — delegate to Scout."

The PRD requirement was:
> daemon dispatch + main agent / orchestration invocation → Scout as a real subagent

Currently Scout is an orphan API.

## Target Behavior

Integrate Scout into the main loop with a lightweight router:

```rust
// In run_until_pause(), before model call:
if should_delegate_to_scout(&objective, &state) {
    let finding = self.run_scout(store, session_id, ScoutRequest {
        scout_id: ScoutId::new(),
        parent_turn_id: turn_id,
        objective: objective.clone(),
        max_actions: 8,
        max_tokens: 32_768,
        allowed_action_kinds: vec!["read".into()],
    }).await?;
    // Inject scout findings as retrieved context for the main turn
    context_hits.extend(scout_finding_to_hits(&finding));
}
```

The router heuristic can start simple: delegate when the objective contains exploration markers ("find", "explore", "investigate", "understand", "how does", "what is") AND the session has no plan yet (plan_steps is empty).

## Implementation Files

| File | Change |
|------|--------|
| `crates/agent-runtime/src/agent.rs` | Add `should_delegate_to_scout()` heuristic; integrate scout dispatch into `run_until_pause()` |
| `crates/agent-runtime/src/context.rs` | Add `scout_finding_to_hits()` to convert `ScoutFinding` into `ContextHit` items |

## Acceptance Criteria

- [ ] Main agent loop can dispatch to Scout before its own model call
- [ ] Scout findings are injected as context for the main turn
- [ ] Scout delegation is observable in the session event log
- [ ] Agent does not delegate to Scout when plan_steps is non-empty (already executing)

---

# P0-8: EvidenceRef Must Use Real Line Ranges and Digests

## Current Defect

`run_scout()` lines 1333-1343:
```rust
evidence.push(EvidenceRef {
    path,
    line_range: (1, 0),  // invalid: start > end
    excerpt: stdout.chars().take(2048).collect(),  // entire stdout, not per-path
});
```

`(1, 0)` is an invalid range (start > end). And if a single read touches multiple paths, the entire stdout is copied to every path's evidence — the same text appears as "evidence" for `auth.rs` and `middleware.rs`.

## Target Behavior

EvidenceRef must come from structured tool output, not from string copying:

```rust
struct ToolEvidence {
    path: PathBuf,
    start_line: u32,
    end_line: u32,
    digest: String,       // BLAKE3 of the actual content
    excerpt: String,
    truncated: bool,
}
```

Claw should return `Vec<ToolEvidence>` for each executed read, containing the actual line ranges and content digests. Scout collects these structured results.

## Implementation Files

| File | Change |
|------|--------|
| `crates/agent-runtime/src/agent.rs` | `EvidenceRef`: fix `line_range` from `(1,0)` to actual range; add `digest` field |
| `crates/claw-sandbox/src/lib.rs` | (Optional, P1) Return structured per-path evidence from typed reads |

## Temporary Fix (P0)

Until Claw returns structured evidence, compute valid line ranges from the read output:

```rust
// For ReadFile: count lines in output, range = (1, line_count)
let line_count = stdout.lines().count().max(1) as u32;
evidence.push(EvidenceRef {
    path,
    line_range: (1, line_count),
    excerpt: stdout.chars().take(2048).collect(),
    digest: blake3::hash(stdout.as_bytes()).to_hex().to_string(),
});
```

## Acceptance Criteria

- [ ] `EvidenceRef.line_range` is `(1, N)` where `N >= 1` — never `(1, 0)`
- [ ] Each path gets its own excerpt, not a copy of the entire stdout
- [ ] EvidenceRef includes a content digest for provenance verification

---

# P0-9: TurnId Must Be Created at User Message Admission

## Current Defect

Daemon appends user messages with `turn_id: None` (lib.rs). `run_until_pause()` creates `TurnId::new()` at iteration start (agent.rs:1670). This means:

- User messages have `turn_id = None`
- Agent actions have `turn_id = abc123`
- Assistant response has `turn_id = abc123`

The work log correlation intended by PRD v1.1 §6.3 breaks: during an active turn, no message carries the TurnId yet, so the UI falls back to "transcript end" positioning. And the final correlation points at the assistant answer, not the user request that triggered the turn.

## Target Behavior

TurnId originates at daemon admission:

```
POST /v1/sessions/{id}/messages
    ↓
TurnId::new()
    ↓
ConversationMessage { role: "user", turn_id: Some(turn_id) }
    ↓
AgentOperation { turn_id }
    ↓
All ProposedAction/ActionOutputRecorded/JudgmentRecorded { turn_id: Some(turn_id) }
    ↓
Assistant ConversationMessage { turn_id: Some(turn_id) }
```

The daemon passes the TurnId into `run_until_pause()` rather than the agent creating it.

## Implementation Files

| File | Change |
|------|--------|
| `crates/purrcode-daemon/src/lib.rs` | Create TurnId at user message admission; pass to `NativeAgent` |
| `crates/agent-runtime/src/agent.rs` | Accept `turn_id: TurnId` parameter in `run_until_pause()` / `continue_turn()` |
| `crates/runtime-core/src/lib.rs` | `ConversationMessage`: set `turn_id` on user messages |

## Acceptance Criteria

- [ ] User `ConversationMessage` carries `turn_id: Some(...)` when created via daemon API
- [ ] Agent receives TurnId from daemon, does not create its own
- [ ] All actions, judgments, and assistant messages in a turn share the same TurnId
- [ ] Work Log correlation finds the user request, not the assistant response

---

# P0-10: Compaction Must Use Token-Based Complete-Turn Retention

## Current Defect

agent.rs:83 — `RETAINED_ACTIONS_AFTER_COMPACTION = 6`

This retains the last 6 messages, which may be:
- 6 assistant-only messages (no user context)
- Partial turns (conversation boundary in the middle)
- Arbitrary token count (could be 1k or 200k)

What's needed is: retain the most recent **complete turns** that fit within a token budget, plus the checkpoint.

## Target Behavior

```rust
fn compaction_window(conversation: &[ConversationMessage], max_tokens: u64) -> usize {
    let mut tokens = 0u64;
    let mut turn_boundaries = Vec::new();
    // Find turn boundaries (user → assistant pairs)
    // Walk backwards, accumulating complete turns
    // Stop when tokens > max_tokens
    // Return the index of the first retained message
}
```

The retained window should start at a user message (turn boundary), not mid-turn. Target: retain the last ~8K tokens of complete turns.

## Implementation Files

| File | Change |
|------|--------|
| `crates/agent-runtime/src/agent.rs` | Replace `RETAINED_ACTIONS_AFTER_COMPACTION` constant with `compaction_window()` function |
| `crates/runtime-core/src/lib.rs` | `CheckpointCompacted`: change `conversation_messages_retained_from` to use turn-boundary-aligned index |

## Acceptance Criteria

- [ ] Retention is token-based, not message-count-based
- [ ] Retained window starts at a user message (turn boundary)
- [ ] Checkpoint + recent complete turns always fit within the token budget

---

---

# P1: Product-Feel Items

## P1-1: Unify Model-Facing Schema to `actions[]` Only

**File:** `crates/agent-runtime/src/schema.rs`, `crates/agent-runtime/src/context.rs`

- Keep `action` field for backward-compat deserialization but remove it from the model-facing prompt
- Developer prompt should show only `actions[]` in the JSON schema example
- Validation: `complete=true` → `actions=[]`; `complete=false` → `actions.len() >= 1`; if any action is mutating → `actions.len() == 1`

## P1-2: Populate Semantic Checkpoint Fields

**File:** `crates/agent-runtime/src/agent.rs` lines 1806-1824

Currently 10 fields are `vec![]` / `None`. Populate:
- `accepted_requirements`: from validated plan steps
- `user_constraints`: from `SessionControls`
- `important_symbols`: from Whisker Tier 1 symbol extraction
- `validated_facts`: from passing validation results
- `unresolved_questions`: from incomplete plan steps
- `current_hypothesis`: from the most recent turn's rationale
- `next_actions`: from remaining plan steps

## P1-3: Broaden FailedAttempt Detection

**File:** `crates/agent-runtime/src/agent.rs` lines 1766-1789

Current `failed_attempts` only captures `judgment != Allow`. Add:
- `FailedAttemptSource::ExecutionFailed` — action executed but `exit_code != 0`
- `FailedAttemptSource::ValidationFailed` — validation after action failed
- `FailedAttemptSource::TestFailed` — test run produced failures
- `FailedAttemptSource::RepairSuperseded` — action was repaired/replaced

## P1-4: Use Model Context Limit, Not Budget

**File:** `crates/agent-runtime/src/agent.rs`, `crates/provider-gateway/src/lib.rs`

Add `context_limit` to `ModelId` or `ModelProvider` trait. Use `min(model.context_limit - output_reserve - safety_buffer, user_budget)` as the effective input capacity — not `maximum_input_tokens` from budget alone.

## P1-5: Context Overflow Auto-Recovery

**File:** `crates/agent-runtime/src/agent.rs`, `crates/provider-gateway/src/lib.rs`

Catch `ProviderErrorCategory::ContextLengthExceeded` (or similar) from provider responses. On overflow:
1. Compact once
2. Rebuild request
3. Retry once
4. If still overflows, fail with `ContextOverflow`

## P1-6: Real `@file` / `#symbol` with Autocomplete

**File:** `crates/purrcode-ide/src/composer.rs` (or TUI equivalent), `crates/purrcode-tui/src/composer.rs`

- `@` triggers Whisker-powered fuzzy file search
- Selected file becomes a `PinnedContextRef` chip
- `#` triggers symbol search
- Chips support: click to pin, × to remove, line range annotation

## P1-7: Pinned Context Chip UI

**File:** IDE/TUI composer component

- Render pinned context as chips below the composer
- Each chip shows: filename, token estimate, × button
- Pins are durably recorded in session state

## P1-8: Context Inspector UI

**File:** IDE/TUI workbench

- Token meter becomes clickable → opens Context Inspector
- Inspector shows per-ContextClass breakdown: Instructions, Task State, Checkpoint, Conversation, Retrieved, Tool Evidence, Reserve
- Each section expandable with `WhyIncluded` provenance
- Data sourced from `GET /v1/sessions/{id}/context-ledger/{turn_id}`

## P1-9: Current-Context Meter ≠ Session Usage Meter

**File:** `crates/purrcode-ide/src/app/workbench.rs` (or equivalent)

- Separate the token meter into two metrics:
  - **Context:** current turn context size / model limit (from context ledger)
  - **Usage:** cumulative session usage (from `UsageLedger`)
- Remove hardcoded `cap = 200_000` — use actual model context limit

## P1-10: Manual `/compact` Command

**File:** `crates/purrcode-cli/src/main.rs`, `crates/purrcode-tui/src/command_palette.rs`

- Slash command `/compact` triggers one compaction cycle immediately
- Available in CLI and IDE command palette
- Shows before/after token counts

## P1-11: `WhyIncluded` for Every Retrieved Hit

**File:** `crates/agent-runtime/src/context.rs`, `crates/whisker-context-engine/src/lib.rs`

Each `ContextHit` should carry a `WhyIncluded` reason:
- `MatchedQuery { term }` — matched BM25/FTS query term
- `ImportProximity { imported_by }` — included because imported by another hit
- `Cochange { changed_with }` — historically changed together
- `TestRelation { tests }` — test file for a source hit

## P1-12: Auto-Context Toggle

**File:** IDE/TUI settings, daemon session controls

- User-toggle: "Auto Context" On/Off
- When Off: only pinned context + conversation + checkpoint sent
- When On: current behavior (Whisker retrieval runs each turn)

## P1-13: Query Normalization for Symbol Search

**File:** `crates/whisker-context-engine/src/` (likely TUI/IDE composer)

- Remove stopwords before symbol search
- Extract code identifiers (camelCase/snake_case splitting)
- Don't just take the first 3 whitespace-split tokens

## P1-14: Import/Cochange/Test Graph Expansion

**File:** `crates/whisker-context-engine/src/`

- After seed candidate retrieval, expand by one hop: imports, co-changes, test relations
- Boost but don't replace seed candidates
- Use the already-existing `cochanges` and `test_relations` data

---

# P2: Future Items

1. Lightweight reranker (cross-encoder or ColBERT-style)
2. Optional embedding-based retrieval
3. Multiple parallel Scouts for different subtrees
4. Scout specialization (security scout, performance scout, i18n scout)
5. Context effectiveness learning/ranking (which retrievals led to edits?)
6. Prefix-cache awareness (structure prompts so Anthropic prompt caching hits)

---

# Implementation Plan

## Phase 1: P0 Release Blockers (this PR)

1. P0-1/P0-2: Rewrite compaction as preflight (single change fixes both)
2. P0-3: ContextEnvelope + final-request ledger
3. P0-4: Per-action batch constraints
4. P0-5: Scout tool result feedback loop
5. P0-6: Scout multi-read support
6. P0-8: EvidenceRef real line ranges
7. P0-9: TurnId at admission
8. P0-10: Token-based turn retention

P0-7 (Scout router) can be a fast-follow if the PR is getting large, since it depends on P0-5 and P0-6 working first.

## Phase 2: P1 Product Polish (next PR)

Items P1-1 through P1-14 as listed above.

## Phase 3: P2 Advanced Features

As prioritized during dogfooding.

---

# Testing Strategy

Each P0 fix includes specific unit tests listed in its section. Additionally:

- **Integration:** `purrcode-tui-e2e` test for the complete compaction → rebuild → model call cycle
- **Regression:** All existing tests must pass (especially `ledger_section_sum_matches_the_aggregate_token_estimate_for_the_same_turn`)
- **Manual verification:** `/compact` in a real session with >70% token utilization

---

# References

- OpenCode V2 Compaction: https://opencode.ai/v2/docs/compaction (preflight estimation, model context limit, checkpoint + recent tail, overflow recovery)
- OpenCode Agents: https://dev.opencode.ai/docs/agents/ (Explore/General/Scout as real subagents, parallel subagent work)
