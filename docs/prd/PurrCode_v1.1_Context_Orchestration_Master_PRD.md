# PurrCode v1.1 Master Product Requirements Document

## Context Orchestration, Tool-Loop Optimization, and Context-Isolated Subagents

**Document status:** Ready for Codex implementation
**Target release:** `v1.1.0`
**Baseline release:** `v1.0.0`
**Baseline commit:** `156d83206ed74a89398c0adf8d4fccd3e070ae59` (branch `main`)
**Working branch:** `feature/v1.1-context-orchestration`
**Primary subject:** `crates/agent-runtime`, `crates/whisker-context-engine`, `crates/runtime-core`, `crates/purrcode-ide`
**Safety subject:** none — `crates/pawgate-runtime`, `crates/claw-sandbox`, `crates/ninelives-recovery` are preserved, not redesigned
**Embeddings/vector search:** explicitly out of scope for this release
**Full IDE editor parity:** explicitly out of scope for this release

---

# 0. Codex Master Goal

Implement PurrCode v1.1 as a context-orchestration and tool-loop rework of the existing agent runtime, without touching PurrCode's safety differentiator.

A reviewer architecture critique of the agent loop, context engine, subagents, IDE, and settings was independently verified claim-by-claim against this codebase by seven separate verification passes (see §2). Every concrete code claim in the critique was CONFIRMED or PARTIALLY CONFIRMED. This PRD turns the confirmed drift into five ordered, shippable phases:

```text
Phase 1 — Context Ledger + Inspector
Phase 2 — Semantic Checkpoint Compaction
Phase 3 — Multi-Read / Single-Mutation Tool Loop
Phase 4 — Whisker Retriever v2
Phase 5 — Context-Isolated Scout Subagent + Context-as-UI-Primitive
```

The reviewer's own prioritization is preserved: this is a **context orchestration and tool-loop** release, not an embeddings/RAG release. Vector search is explicitly deferred (§4.2). The reviewer's own normative IDE opinion — "stop chasing VS Code parity, double down on agent/context/changes/evidence/terminal-first" — is treated as a design direction endorsed by this PRD, not as a verified defect; it shapes Non-Goals (§4.1) and Phase 5's UI scope (§10.5), nothing more.

The non-negotiable constraint across all five phases:

```text
PawGate authorization must remain per-action, synchronous, and pure.
Claw execution must remain typed, single-use-authorization, and durably logged.
NineLives must remain the single source of truth: append-only, replayable, no data loss.
```

Every phase below states explicitly how it preserves these three invariants. None of the five phases adds a new authorization surface, a new durable store, or a bypass of PawGate/Claw. Phase 3 is the one phase that touches the shape of an authorized action (single action → action set); §8.5 states exactly how PawGate and Claw absorb that change without a redesign.

---

# 1. Baseline Rule

PurrCode v1.0 already implements the full agent loop, context engine, subagent roles, IDE workbench, and settings surface described in the v1.0 PRD. The following are compatibility invariants and must not be reimplemented from scratch or weakened:

- `AgentTurn` / `validate_turn` as the model-facing turn contract (`crates/agent-runtime/src/schema.rs`);
- structured-JSON turns with `tools: Vec::new()` on every `ModelRequest` — **no provider-native tool calling is introduced by this PRD**, in Phase 3 or anywhere else;
- `Policy::evaluate` as PawGate's pure, per-action, stateless authorization function (`crates/pawgate-runtime/src/lib.rs`);
- `ToolRuntime::execute` and single-use authorization consumption as Claw's execution contract (`crates/claw-sandbox/src/lib.rs`);
- `SessionStore::append` / `reduce_event` / `load` / `events` as NineLives' durable, replayable event log (`crates/ninelives-recovery/src/lib.rs`);
- the `run_until_pause` / `run_planner` iteration model in `crates/agent-runtime/src/agent.rs`;
- the `SessionState` reducer in `crates/runtime-core/src/lib.rs` as the single authoritative in-memory projection of the event log;
- `whisker-context-engine`'s FTS5 `chunks` table and `retrieve()` as the retrieval entrypoint;
- the IDE workbench (`crates/purrcode-ide/src/app/workbench.rs`), settings (`crates/purrcode-ide/src/app/settings.rs`), and daemon control lane (`crates/purrcode-daemon/src/lib.rs`).

Codex must extend these contracts. Codex must not replace them with a parallel context system, a second authorization path, or a second event log.

---

# 2. Verified Baseline

Seven independent verification passes read the actual source on `feature/v1.1-context-orchestration` (identical to `main` at `156d832` for every file cited) and confirmed or refuted each concrete claim in the reviewer's critique. This section states, per area, what is actually true today. Nothing marked `NOT_CONFIRMED` exists in this PRD — there were none; every claim area below is `CONFIRMED` or `PARTIALLY_CONFIRMED`, and the partial nuances are stated precisely because they change what Codex should build.

## 2.1 Agent tool loop — CONFIRMED

`AgentTurn` carries at most one action:

```rust
// crates/agent-runtime/src/schema.rs:19-29
pub struct AgentTurn {
    pub action: Option<AgentAction>,
    pub complete: bool,
    // ...
}
```

`validate_turn` enforces XOR between completion and action (`schema.rs:200-204`):

```rust
if turn.complete == turn.action.is_some() {
    return Err(/* "exactly one of complete=true or action must be supplied" */);
}
```

The prompt instructs the model accordingly, verbatim (`crates/agent-runtime/src/context.rs:780-781`):

> "CRITICAL: If complete=false, provide EXACTLY ONE action. If complete=true, rationale MUST be the concrete answer — NOT a progress note..."

Every `ModelRequest` built by `NativeAgent` sets `tools: Vec::new()` — confirmed at both call sites, `agent.rs:1580-1586` (`coding_worker`) and `agent.rs:1090-1096` (`planner`). A provider emitting a native `ModelEvent::ToolCall` during a structured call is treated as a hard protocol error by `structured_observed_from_tracker` (`agent.rs:628-650`, `"structured provider response unexpectedly requested a tool"`). `run_until_pause` (`agent.rs:1440-1624`, `MAX_AUTONOMOUS_ITERATIONS = 32`) therefore costs one full model round-trip per action — a five-step exploration (`grep`+`read`+`read`+`read`+`diff`) costs five round-trips today.

The two safety primitives this constraint interacts with are both individually action-scoped already, which is exactly what Phase 3 needs:

- `Policy::evaluate(&self, action: &ProposedAction, repository: &Path) -> JudgmentDecision` (`crates/pawgate-runtime/src/lib.rs:81-213`) is pure and stateless per action — calling it N times for N actions requires no PawGate change.
- `ToolRuntime::execute(store: &mut SessionStore, action_id, action: &ProposedAction, constraints)` (`crates/claw-sandbox/src/lib.rs:74-116`) takes an **exclusive** `&mut SessionStore` and consumes a single-use authorization per call — literal concurrent calls against one store handle are not expressible today. `execute_typed_read` (`lib.rs:119-313`), the read-path logic itself, has no shared mutable state and is naturally parallelizable. This is a real, unsolved wiring gap, not a design objection: Phase 3 needs a new batch-execute entrypoint, not "call `execute()` in a loop."

## 2.2 Context compaction — CONFIRMED

```rust
// crates/agent-runtime/src/agent.rs:79-80
const MAX_ACTIONS_IN_PROMPT: usize = 12;
const RETAINED_ACTIONS_AFTER_COMPACTION: usize = 6;
```

The trigger (`agent.rs:1440-1474`) is a raw count of `state.proposed_actions`, not a token estimate — even though a real token-accounting path already exists and goes unused for this decision:

```rust
// crates/agent-runtime/src/agent.rs:277-315 (prepare_model_request)
// provider.count_tokens(&request); tracks input_so_far/output_so_far/total_so_far
// against budget.maximum_input_tokens/maximum_output_tokens/maximum_total_tokens
```

`SessionState` (`crates/runtime-core/src/lib.rs:1040,1049`) has two structurally independent fields:

```rust
pub context_summary: Option<String>,
pub conversation_messages: Vec<ConversationMessage>,
```

The `ContextCompacted` reducer arm **overwrites** `context_summary` and prunes `proposed_actions`/`judgments`/`contextual_judgments` to the retained set — it never touches `conversation_messages` (`runtime-core/src/lib.rs:1425-1435`). `conversation_messages` is grown only by `ConversationMessageAdded` (`lib.rs:1443-1445`) and is never pruned by anything. `build_messages()` unconditionally includes the full `conversation_messages` on every call (`context.rs:743-751`) regardless of whether compaction has run. So compaction trims one growth vector (action/judgment history) while leaving the turn-by-turn transcript completely unbounded — this is the real, confirmed drift.

The summary text itself is deterministic Rust formatting, not an LLM call, and it is a flat string that is replaced (not merged) on every compaction (`agent.rs:1451-1468`, `runtime-core/lib.rs:1430`):

> `"Compacted {compacted} older actions. Across the pre-compaction window, {successful} actions had allow-class deterministic/effective judgments. The durable event log remains authoritative."`

It reports only a count of allow-class judgments — no failed attempts, no files touched, no decisions. Because it is overwritten, a second compaction discards whatever the first one captured beyond the newly retained 6 actions.

NineLives already retains every raw `SessionEvent` forever and can replay it (`crates/ninelives-recovery/src/lib.rs:71-101,226,241` — `SessionStore::append` persists full JSON payloads append-only; `load()`/`events()` replay/return the complete history). A structured checkpoint can be a derived projection over this log with zero new storage, exactly as the reviewer assumed.

## 2.3 Retrieval — PARTIALLY CONFIRMED

All numeric/mechanical claims hold verbatim:

```rust
// crates/whisker-context-engine/src/lib.rs:19-20
const CHUNK_LINES: usize = 100;
const CHUNK_OVERLAP: usize = 10;
```

`chunks()` (`lib.rs:1635-1651`) is pure fixed-size line-window chunking with zero syntax awareness. `RetrievalBudget::default()` (`lib.rs:390-397`) is `maximum_hits: 12, maximum_bytes: 64 * 1024`. `retrieve()`'s SQL (`lib.rs:941-953`) ranks by exactly four signals: `bm25(chunks)*1000` + path/filename substring bonus (`+5000`) + changed-file bonus (`+2500`) + git-recency term (`last_commit/1_000_000`). A "widen to whole repo" fallback exists (`lib.rs:676-686`) when Tier1 file selection comes up empty — it lives in `index_tier1`'s file-selection step, one layer above `retrieve()` itself, and triggers on zero *total* selection (paths ∪ directory prefixes ∪ filename terms ∪ requested languages), not narrowly "zero filename hits."

The nuance that changes Phase 4's design: the critique frames this as "no symbol/AST awareness." That undersells what already exists. `tree-sitter` is wired up for six languages, with a real AST walk populating a separate `symbols` table (`lib.rs:12,1663-1730`, `extract_symbols`/`collect_symbols`, grammars for Rust/Python/TypeScript/JavaScript/Java/Go). An `imports` table is likewise populated at index time (~`lib.rs:1142-1148`). But `retrieve()` (`lib.rs:929-986`) — the sole retrieval entrypoint — queries only `chunks` LEFT JOIN `git_files`; it never references `symbols` or `imports`. So the accurate framing is: **AST-derived signals already exist as unfused side-channels; the fix is fusion into ranking, not building symbol extraction from scratch.** Chunk *boundaries* remain naive line windows regardless. No vector/embedding code exists anywhere in the crate (grep-clean), consistent with the reviewer's own deprioritization of it.

## 2.4 Prompt assembly / token budget — CONFIRMED

`build_messages()` (`context.rs:605-788`) concatenates, unconditionally, on every call: one static ~140-line developer instructions block; the full `state.conversation_messages`; then one final user message with, in order, `## CURRENT REQUEST` (duplicating `objective`), worktree, current plan, recent actions/results (from `proposed_actions`+judgments+outputs), last-8 validation events, retrieved repository context, `COMPACTED PRIOR CONTEXT:{context_summary}`, and the full JSON output-format/typed-action schema. `agent.rs` also splices an "EFFECTIVE DAEMON CONTRACT" system message (`1535-1556`) and, near the iteration cap, a "STEP LIMIT WARNING" (`1562-1578`). None of these sections are gated by task type, token estimate, or importance.

The duplication is not incidental to `build_messages()` — it is structural, coming from the daemon: session creation (`crates/purrcode-daemon/src/lib.rs:1896-1909`) and follow-ups (`lib.rs:2017-2038`) both durably append the user's exact text as `ConversationMessageAdded` **before** the agent runs; `agent.rs` then derives `objective` either from that same text or by reading the last "user" entry back out of `conversation_messages` (`agent.rs:1486-1497`). So the latest user message is guaranteed to appear twice in every prompt, on every turn, by construction.

`BudgetConstraints` (`crates/runtime-core/src/adaptation.rs:233-243`) is a flat struct of scalar caps — no per-context-class breakdown:

```rust
pub struct BudgetConstraints {
    maximum_input_tokens, maximum_output_tokens, maximum_total_tokens,
    maximum_estimated_cost, maximum_model_calls, maximum_search_requests,
    maximum_mcp_calls, maximum_wall_time_seconds,
}
```

It is checked once, in aggregate, after full assembly (`agent.rs:265-317`, single `provider.count_tokens(&request)` call) — never as a pre-assembly allocator. `max_output_tokens: Some(4096)` is a hardcoded literal at both `model_for` call sites (`agent.rs:1094,1584`), not adapted to task class.

## 2.5 Subagents — PARTIALLY CONFIRMED

The critique's bottom line — no Explore/Scout-style subagent exists that does its own multi-step read-only exploration and returns only compact structured findings — is CONFIRMED. A repo-wide grep for scout/explore-agent/isolated-context patterns finds nothing.

The framing that named roles are "workflow-role routing, not context-isolated exploration" needs correction:

- `ModelRole` (`crates/model-selection/src/lib.rs:34-58`) has exactly six variants: `CodingWorker, Planner, Judge, Summarizer, Embedding, FastRouter`. `reviewer` and `utility` are **not** in this enum — they exist only as loose routing-label strings recognized by `provider-gateway`'s `canonical_model_role()` (`crates/provider-gateway/src/lib.rs:987-989`) and as settings UI slots (`crates/purrcode-ide/src/app/settings.rs:~85-90`), with **zero** `model_for("reviewer"|"utility")` call sites anywhere.
- `model_for(...)` in `agent-runtime` is called only with `"planner"` (once, in `run_planner`) and `"coding_worker"` (repeatedly, in the main loop) — confirmed by grep across `agent.rs:1081,1520-1852`.
- `summarizer` is a real `ModelRole` with UI/config plumbing but zero call sites — compaction (§2.2) is deterministic string formatting, no LLM call at all, so "summarizer" does not summarize anything today.
- `run_planner()` (`agent.rs:1053-1144`) and `ContextualJudge::evaluate()` (`crates/contextual-judgment/src/lib.rs:34-70`) genuinely run in **isolated** model calls with fresh, bounded, purpose-built payloads — not the shared `conversation_messages`/`build_messages()`. They are isolated but not exploratory: each consumes evidence someone else already gathered (`run_planner` issues exactly one `retrieve()` call; it does not loop).
- The real precedent for "isolated context + condensed return" is `JudgedSupervisorWorker`/`supervisor-runtime` (`purrcode-daemon/src/lib.rs:1051-1256`, `supervisor-runtime/src/lib.rs:1-70`): each parallel worker runs in its own git worktree, its own single-turn `AgentTurn`-schema conversation, and returns only `WorkerOutput{summary, model_requests}`. It is architecturally close to what a Scout needs but is built for **parallel decomposed coding subtasks** (workers write code, need PawGate merge review, have dependency/merge semantics) — a read-only Scout should reuse the shape (isolated conversation, condensed return) and skip the worktree/merge machinery entirely.

## 2.6 IDE workbench UX — CONFIRMED

```rust
// crates/purrcode-ide/src/app/workbench.rs:254-260
// Runtime activity belongs between the user's request and
// the final answer. We do not have per-turn activity IDs
// yet, so the log is anchored to the request instead — ...
```

```rust
// crates/purrcode-ide/src/app/workbench.rs:1315-1317
fn work_log_anchor(messages: &[model::Message]) -> Option<usize> {
    messages.iter().rposition(model::Message::is_user)
}
```

A crate-wide grep for `TurnId|SpanId|ToolCallId|ParentSpanId|turn_id|span_id|tool_call_id` finds exactly the one comment above — no such types exist. `Message` (`crates/purrcode-ide/src/model.rs:36-40`) and `ActivityLine` (`model.rs:48-54`) carry no id/turn/span field of any kind.

The composer (`workbench.rs:615-716`) is a single `egui::TextEdit::multiline` plus a bypass chip and send button. The `"/ commands   @ files   # symbols"` text (`workbench.rs:657-663`) is a static placeholder shown only when the composer is empty — no code path parses `@` tokens, resolves file references, renders pinned-context chips, or computes any token-usage breakdown (a separate grep for `token_usage|usage_meter|context_budget|tokens_used|prompt_tokens|completion_tokens` returns zero hits; the crate's only `token` hits are an unrelated design-system `self.tokens` struct for colors/radii).

The reviewer's recommendation to stop chasing VS Code parity and double down on agent/context/changes/evidence/terminal-first is a design opinion, not a verifiable code defect — treated here as direction, not as confirmed drift (see §0, §4.1).

## 2.7 Settings — CONFIRMED (tracked separately, not scheduled in this PRD)

```rust
// crates/purrcode-ide/src/app/settings.rs:169-197
pub(crate) enum SettingsPage {
    General, Models, LocalModels, Skills, Mcp, Codex, Authority, Agent, Terminal, Privacy, Advanced,
}
```

`group()` (`settings.rs:215-223`) buckets these 11 pages into 5 visual headers (WORKSPACE/MODELS/EXTENSIONS/RUNTIME/SYSTEM) — grouped, not literally flat — but there is no basic/advanced *disclosure*: MCP, Skills, Codex, and Agent (routing/budgets) are peers of General in the one default scrolling list. `SettingsState` (`settings.rs:97-146`) has 20+ fields, almost all `serde_json::Value`/`Vec<Value>`. A single `pending: Option<String>` (`settings.rs:139-166`) attributes the transport's one generic `Response::Failed(String)` (`crates/purrcode-ide/src/daemon.rs:557-586`) back to whichever mutation is currently in flight (`crates/purrcode-ide/src/app/mod.rs:1267-1279`) — the code's own comment admits this "stays accurate in practice" only because the control lane is serial.

This file already carries its own "Defect A (PRD §1)" comment header, i.e. this gap is a previously-tracked v1.0 item, not new. **This PRD does not schedule settings progressive disclosure** — it is out of the reviewer's five-phase context-orchestration roadmap and is left to its existing tracking (see §4.3).

---

# 3. Product Vision for v1.1

PurrCode v1.0 shipped a working agent loop, a working lexical retrieval engine, and a working safety chain. v1.1 does not replace any of that. It fixes the specific, verified shape of context PurrCode sends to the model and the specific, verified shape of how the model is allowed to act on it, so that:

> The agent explores in bursts instead of one action per round-trip, remembers what has already failed instead of forgetting it at the first compaction, retrieves with the signals it already computes instead of half of them, keeps large exploration out of the primary conversation instead of inlining every grep result, and lets the person supervising the agent see — and shape — what the model is being shown.

This is a context orchestration and tool-loop release. It is deliberately not a retrieval-quality-via-embeddings release (§4.2) and deliberately not an editor-parity release (§4.1).

---

# 4. Non-Goals

## 4.1 Full VS-Code-parity IDE work

Out of scope for v1.1:

- a general-purpose code editor competitive with VS Code's editing feature set;
- new language-server integrations beyond what already exists;
- new editor panes unrelated to agent supervision (extensions marketplace, generic multi-root workspace management, etc.).

Why: the reviewer's own recommendation (§2.6, treated as direction) and this PRD's own priorities agree that PurrCode's differentiated surface is *supervising an autonomous agent* — context, changes, evidence, terminal — not competing with a mature general-purpose editor on editing features. The only IDE work scheduled in this PRD is Phase 5's composer context primitives (§10.5), which are agent-supervision UI, not editor UI.

## 4.2 Embeddings / vector search

Out of scope for v1.1, deferred to a later, unscheduled phase:

- vector embedding generation or storage;
- ANN/vector similarity retrieval;
- any hybrid BM25+vector reranker.

Why: §2.3 confirms no vector code exists today and confirms that the highest-leverage retrieval fix is fusing signals PurrCode **already computes** (`symbols`, `imports`) into `retrieve()`, plus optional node-boundary chunking — a wiring/fusion problem, not a new-subsystem problem. This is materially lower risk and lower cost than standing up an embedding pipeline, and it is the reviewer's own stated priority order. Vector search remains a legitimate future phase once lexical+symbol+import fusion (Phase 4) is shipped and measured (§13).

## 4.3 Settings progressive disclosure

Out of scope for v1.1 (§2.7). `crates/purrcode-ide/src/app/settings.rs` already carries its own tracked "Defect A" comment from v1.0's own PRD history; it is not part of the reviewer's five-phase context-orchestration roadmap and should be scheduled independently rather than folded into this release.

## 4.4 Native provider tool-calling

Out of scope, explicitly, for this PRD's Phase 3. Phase 3 changes how many actions one structured `AgentTurn` may carry — it does not turn on `tools: Vec::new()` → real provider tool schemas, and it does not change `structured_observed_from_tracker`'s treatment of `ModelEvent::ToolCall` as a protocol error (§2.1, §8.4). The reviewer explicitly asked to keep PawGate/Claw's safety model, not adopt native tool use; this PRD holds that line.

---

# 5. Roadmap Overview

```text
Phase 1  Context Ledger + Inspector
         → gives every turn/action/context-section a durable identity and
           a per-section token account, so every later phase is measurable
           and the IDE can render real provenance instead of guesses.

Phase 2  Semantic Checkpoint Compaction
         → replaces the action-count trigger and the lossy overwritten
           summary string with a token-aware trigger and a structured,
           additive checkpoint whose failed_attempts field survives
           repeated compaction.

Phase 3  Multi-Read / Single-Mutation Tool Loop
         → lets one structured AgentTurn carry a set of read-only actions,
           validated individually by PawGate and executed concurrently by
           a new Claw batch entrypoint; mutating actions stay exactly as
           constrained as they are today (exactly one per turn).

Phase 4  Whisker Retriever v2
         → fuses the symbols/imports signals whisker-context-engine
           already computes but never queries into retrieve()'s ranking,
           and optionally moves chunk boundaries toward AST node
           boundaries for tree-sitter-supported languages.

Phase 5  Context-Isolated Scout Subagent + Context-as-UI-Primitive
         → adds a genuinely new isolated, multi-step, read-only exploration
           subagent (built on Phase 3's action-set loop, not on the
           worktree/merge-coupled supervisor path), and surfaces Phase 1's
           ledger in the IDE composer as context chips, a token-class
           usage meter, and "why included" provenance.
```

Ordering rationale: Phase 1 is instrumentation-first because Phases 2-5 all need to be measured against a real baseline (§13), and because Phase 5's UI work is otherwise unbuildable. Phase 3 (the tool loop) is the reviewer's own stated "single biggest latency/token/context-quality problem" and is sequenced before retrieval quality work because it changes the unit of work retrieval needs to serve (an action set, not a single action). Phase 4 and Phase 5 are independent of each other and could ship in either order; Phase 5 is listed last because it is additive on top of Phase 3's plumbing.

---

# 6. Phase 1 — Context Ledger + Inspector

## 6.1 Goal

Give every context-assembly decision, and every model turn, a durable and correlatable identity, and account for prompt tokens *by section* instead of only in aggregate. This closes §2.6's confirmed gap (`Message`/`ActivityLine` have no id; `work_log_anchor` is a positional heuristic) and §2.4's confirmed gap (a single `count_tokens()` call over the whole assembled request, no per-section breakdown) with one mechanism.

## 6.2 Data structures

```rust
// crates/runtime-core/src/lib.rs — new newtypes, alongside existing SessionId/ActionId
pub struct TurnId(pub Uuid);
pub struct SpanId(pub Uuid);
pub struct ToolCallId(pub Uuid);

pub struct ContextLedgerSection {
    pub class: ContextClass,
    pub label: String,           // e.g. "conversation_messages[42..47]"
    pub estimated_tokens: u64,
    pub byte_len: usize,
    pub why_included: WhyIncluded,
}

pub enum ContextClass {
    Instructions,
    ConversationTail,
    TaskState,          // plan, recent actions/results, validation
    RetrievedContext,    // whisker-context-engine hits
    CompactedCheckpoint, // Phase 2's SemanticCheckpoint
    ToolEvidence,
    Reserve,
}

pub enum WhyIncluded {
    AlwaysPresent,
    MatchedQuery { term: String },
    RecentEdit,
    Pinned,
    RetrievedByScout { scout_id: ScoutId }, // wired in Phase 5
}

pub struct ContextLedgerEntry {
    pub turn_id: TurnId,
    pub session_id: SessionId,
    pub sections: Vec<ContextLedgerSection>,
    pub total_estimated_tokens: u64,
    pub recorded_at: DateTime<Utc>,
}
```

New durable event, appended exactly like every other `SessionEvent` today:

```rust
// crates/runtime-core/src/lib.rs — new SessionEvent variant
SessionEvent::ContextAssembled { entry: ContextLedgerEntry }
```

## 6.3 Functions and files to change

- `crates/runtime-core/src/lib.rs`: add `TurnId`/`SpanId`/`ToolCallId`, `ContextLedgerEntry` and friends, `SessionEvent::ContextAssembled`, and a reducer arm that appends the entry to a bounded `SessionState.recent_context_ledger: VecDeque<ContextLedgerEntry>` (bounded independently of Phase 2's compaction — this is inspector data, not model-facing context).
- `crates/agent-runtime/src/agent.rs`: thread a `TurnId` through each `run_until_pause` iteration (`agent.rs:1440-1624`) and through `run_planner` (`agent.rs:1053-1144`); stamp every `ProposedAction`/`ActionOutputRecorded`/`JudgmentRecorded` emitted in that iteration with the same `turn_id`.
- `crates/agent-runtime/src/context.rs`: `build_messages()` (`context.rs:605-788`) computes a `ContextLedgerSection` per section it assembles (developer instructions, conversation tail, plan, recent actions, validation, retrieved context, compacted checkpoint) as it builds them, and returns `(Vec<ModelMessage>, ContextLedgerEntry)` instead of just `Vec<ModelMessage>`. The per-section `estimated_tokens` must be computed with the same `provider.count_tokens`-compatible estimator `prepare_model_request` already uses (`agent.rs:277-315`), so the ledger's sum and the existing aggregate check in `enforce_budget_before_send` never drift.
- `crates/purrcode-daemon/src/lib.rs`: new presentation endpoint `GET /v1/sessions/{id}/context-ledger/{turn_id}`.
- `crates/purrcode-ide/src/model.rs`: add `turn_id: TurnId`, `span_id: Option<SpanId>`, `parent_span_id: Option<SpanId>` to `Message` (`model.rs:36-40`) and `ActivityLine` (`model.rs:48-54`).
- `crates/purrcode-ide/src/app/workbench.rs`: replace `work_log_anchor` (`workbench.rs:1315-1317`) with exact `turn_id` correlation; delete the "we do not have per-turn activity IDs yet" comment (`workbench.rs:254-260`) once the underlying gap it documents is closed.

## 6.4 PawGate / NineLives rollout

No new authorization surface. `ContextAssembled` is one more `SessionEvent` variant flowing through the exact same `SessionStore::append`/`reduce_event`/`load` path every other event already uses (`crates/ninelives-recovery/src/lib.rs:71-101`) — durability and replay guarantees are identical to today's, by construction, because nothing about NineLives' storage or replay logic changes. PawGate and Claw are untouched: this phase adds observability over data that already flows through them, not a new decision point.

## 6.5 Acceptance criteria

- Every `ActivityLine` rendered in the IDE Work Log carries a real `turn_id`/`span_id` pair, not a positional guess; `work_log_anchor`'s heuristic and its explanatory comment are both gone.
- `GET /v1/sessions/{id}/context-ledger/{turn_id}` returns section-by-section token/byte counts whose sum equals the value `enforce_budget_before_send` already computes for that same turn — asserted by a regression test, not eyeballed.
- Replaying a session's NineLives event log reconstructs identical `ContextLedgerEntry` values (durability parity with existing `SessionEvent` replay).

---

# 7. Phase 2 — Semantic Checkpoint Compaction

## 7.1 Goal

Fix the three concrete gaps in §2.2: an action-count trigger with a real-but-unused token accounting path sitting right next to it; a flat, overwritten `context_summary: Option<String>` that loses `failed_attempts` on the second compaction; and a `conversation_messages` growth vector that compaction never touches.

## 7.2 Data structures

```rust
// crates/runtime-core/src/lib.rs — replaces context_summary: Option<String>
pub struct SemanticCheckpoint {
    pub checkpoint_id: CheckpointId,
    pub turn_id: TurnId,               // Phase 1
    pub superseded_checkpoint_id: Option<CheckpointId>, // chain, never overwrite
    pub objective: String,
    pub accepted_requirements: Vec<String>,
    pub user_constraints: Vec<String>,
    pub decisions: Vec<CheckpointDecision>,
    pub files_inspected: Vec<PathBuf>,
    pub files_modified: Vec<PathBuf>,
    pub important_symbols: Vec<String>,
    pub validated_facts: Vec<String>,
    pub failed_attempts: Vec<FailedAttempt>,   // must survive every subsequent compaction
    pub test_results: Vec<TestResultSummary>,
    pub unresolved_questions: Vec<String>,
    pub current_hypothesis: Option<String>,
    pub next_actions: Vec<String>,
    pub pinned_context: Vec<PinnedContextRef>, // wired to Phase 5's composer chips
}

pub struct FailedAttempt {
    pub action_id: ActionId,
    pub action_summary: String,
    pub reason: String,
    pub judgment: Option<String>,
}

pub struct CheckpointDecision {
    pub summary: String,
    pub action_id: Option<ActionId>,
}
```

New event, additive by construction (the reducer appends/merges, it does not `Some(summary.clone())`-overwrite the way `runtime-core/lib.rs:1430` does today):

```rust
SessionEvent::CheckpointCompacted {
    checkpoint: SemanticCheckpoint,
    retained_action_ids: BTreeSet<ActionId>,
    conversation_messages_retained_from: usize, // index into conversation_messages
}
```

## 7.3 Functions and files to change

- `crates/agent-runtime/src/agent.rs`: `agent.rs:79-80` gains a token-based threshold constant (e.g. `const COMPACTION_INPUT_TOKEN_THRESHOLD_RATIO: f64 = 0.7;`, applied against `budget.maximum_input_tokens`) alongside — not necessarily replacing — `MAX_ACTIONS_IN_PROMPT`; the trigger in `run_until_pause` (`agent.rs:1440-1474`) checks the Phase 1 ledger's `total_estimated_tokens` (or a cheap re-estimate of it) against that threshold in addition to the existing count guard, so either condition can fire compaction. Building `checkpoint.failed_attempts` reads the about-to-be-pruned `judgments`/`contextual_judgments` for non-allow-class decisions instead of just counting them.
- `crates/runtime-core/src/lib.rs`: `SessionState` gains `checkpoint: Option<SemanticCheckpoint>` (replacing `context_summary: Option<String>`, `lib.rs:1040`); the `CheckpointCompacted` reducer arm (replacing `ContextCompacted`'s arm at `lib.rs:1425-1435`) sets `checkpoint = Some(merge(old_checkpoint, new_checkpoint))` — `failed_attempts`/`decisions`/`files_inspected` are unioned across the chain via `superseded_checkpoint_id`, never dropped — and additionally truncates `conversation_messages` to `[conversation_messages_retained_from..]`.
- `crates/agent-runtime/src/context.rs`: `build_messages()` (`context.rs:605-788`) renders the structured `checkpoint` fields in place of the current raw `COMPACTED PRIOR CONTEXT:{compacted_context}` string, with `failed_attempts` rendered prominently enough that the model does not re-propose a known-failed approach — this is the reviewer's explicit motivating requirement.
- `crates/ninelives-recovery/src/lib.rs`: no structural change — `CheckpointCompacted` persists through the existing `append`/`reduce_event` path like any other event.

## 7.4 PawGate / NineLives rollout

NineLives already retains every raw `SessionEvent` forever (§2.2, `ninelives-recovery/src/lib.rs:71-101,226,241`) — the checkpoint is a derived, replayable projection over that log, not a replacement for it; full audit fidelity is unchanged. PawGate's judgment retention (`judgments`/`contextual_judgments` pruning in the reducer) keeps pruning to the retained-action set exactly as today; the only change is that `failed_attempts` now captures the pruned judgments' outcomes into the checkpoint before they leave `SessionState`, instead of discarding them into a bare count.

## 7.5 Acceptance criteria

- Regression test: trigger two consecutive compactions in one session; assert `failed_attempts` entries recorded before the *first* compaction are still present (via `superseded_checkpoint_id` chain) in the checkpoint used to build the prompt for the *third* turn. This is the single most important behavioral assertion in this phase — it is currently false (§2.2).
- Unit test: push `proposed_actions` below `MAX_ACTIONS_IN_PROMPT` (≤ 12) but past the token-threshold ratio; assert compaction still fires.
- `conversation_messages.len()` is bounded after compaction in a long-running session (currently unconditionally unbounded, §2.2).

---

# 8. Phase 3 — Multi-Read / Single-Mutation Tool Loop

## 8.1 Goal

Address the reviewer's stated single biggest latency/token/context-quality problem (§2.1): let one structured `AgentTurn` carry a set of read-only actions in one round-trip, while keeping mutating actions exactly as constrained as they are today (exactly one per turn, PawGate-judged, single-use-authorized). This is "parallel read, serialized mutation," not native provider tool calling (§4.4).

## 8.2 Data structures

```rust
// crates/agent-runtime/src/schema.rs — AgentTurn changes shape
pub struct AgentTurn {
    pub actions: Vec<AgentAction>,   // was: pub action: Option<AgentAction>
    pub complete: bool,
    // ... unchanged fields
}
```

New validation rule in `validate_turn` (replacing the XOR at `schema.rs:200-204`, preserving its spirit):

```text
exactly one of:
  complete == true  AND  actions.is_empty()
  complete == false AND  actions is non-empty AND
      ( actions.len() == 1
        OR every action in actions is read-only )
A mutating action (write / patch / delete / run-command) may never
appear alongside any other action in the same turn.
```

```rust
pub enum ActionClass { ReadOnly, Mutating }
// classification is a pure function over the existing ProposedAction/AgentAction
// kind enum — grep/read/list/git-log-style variants are ReadOnly, everything
// that touches the filesystem or a process is Mutating.
```

New Claw entrypoint (`crates/claw-sandbox/src/lib.rs`), alongside the existing `ToolRuntime::execute`:

```rust
impl ToolRuntime {
    pub fn execute_batch(
        store: &mut SessionStore,
        action_ids: &[ActionId],
        actions: &[ProposedAction],   // all ReadOnly, enforced by caller
        constraints: &Constraints,
    ) -> Result<Vec<ActionOutcome>, ClawError> {
        // 1. consume every authorization up front, in order (store.consume_authorization
        //    per action) — still a single &mut SessionStore borrow, still serialized.
        // 2. run execute_typed_read (lib.rs:119-313) concurrently per action — no
        //    shared mutable state, confirmed safe by verification (§2.1).
        // 3. store.append() each ActionOutputRecorded sequentially, in original
        //    action order, regardless of I/O completion order.
    }
}
```

## 8.3 Functions and files to change

- `crates/agent-runtime/src/schema.rs`: `AgentTurn.action` → `AgentTurn.actions: Vec<AgentAction>`; `validate_turn` rewritten per §8.2.
- `crates/agent-runtime/src/context.rs`: `context.rs:780-781`'s prompt text changes from *"provide EXACTLY ONE action"* to *"provide one action, or a set of read-only actions (e.g. grep, read, list, git-log) in a single turn; any mutating action (write, patch, delete, run-command) must be alone in its turn."* The typed-action reference block gains an explicit read-only/mutating tag per action kind.
- `crates/agent-runtime/src/agent.rs`: `run_until_pause` (`agent.rs:1440-1624`) submits an action set to PawGate (looping `Policy::evaluate` once per action — no PawGate signature change, §2.1) and, when every action in the set is `ReadOnly`, calls `claw_sandbox::ToolRuntime::execute_batch` instead of `execute`; mutating turns keep calling `execute` exactly as today. `structured_observed_from_tracker` (`agent.rs:628-650`) is unchanged — it still hard-errors on a native `ModelEvent::ToolCall`, because this remains a structured-JSON turn, not provider tool use (§4.4). `ModelRequest.tools` stays `Vec::new()` at both call sites (`agent.rs:1580-1586`, `1090-1096`) — unchanged.
- `crates/pawgate-runtime/src/lib.rs`: no signature change to `Policy::evaluate`. Optionally add a thin `evaluate_batch` convenience wrapper for tracing/logging symmetry — not required for correctness.
- `crates/claw-sandbox/src/lib.rs`: add `execute_batch` and `ActionClass`/`is_read_only` classification as in §8.2.
- `crates/runtime-core/src/lib.rs`: `SessionEvent::ActionProposed` bookkeeping becomes plural-aware where needed (a turn now proposes 1..N actions instead of 0..1) — `proposed_actions` stays a per-`ActionId` map, so the reducer itself needs no structural change beyond accepting multiple `ActionProposed` events per `turn_id`.

## 8.4 What does not change

`ModelRequest.tools` stays `Vec::new()`. `structured_observed_from_tracker`'s hard error on `ModelEvent::ToolCall` stays. This is still one structured JSON `AgentTurn` response per model round-trip — the only change is that the response's `actions` array may now hold more than one *read-only* entry. §4.4 restates this as an explicit non-goal so it is not conflated with adopting native provider tool-calling.

## 8.5 PawGate / NineLives rollout

`Policy::evaluate` stays synchronous, per-action, and stateless (§2.1) — every action in an action set is still independently judged, and every judgment is still its own durable `JudgmentRecorded` event, so audit granularity (one event per action) is unchanged even though N actions now originate from one model turn instead of N. Mutating actions keep today's exact path unchanged: single action per turn, single-use authorization, serialized `store.append`. Concurrency is opt-in only for the `ReadOnly` class, and only inside Claw's I/O layer (`execute_typed_read` has no shared mutable state, §2.1) — the `SessionStore` append layer stays serialized in both the batch and single-action paths, so NineLives' append-only ordering guarantee is untouched.

## 8.6 Acceptance criteria

- A benchmark task that today issues `grep`→`read`→`read`→`read`→`diff` as 5 sequential model round-trips (§2.1) completes in materially fewer round-trips, while still producing 5 durably logged `ActionProposed`/`JudgmentRecorded` event pairs — one per action, exactly as today.
- A turn mixing a mutating action with any other action is rejected by `validate_turn` with a clear error, mirroring the existing XOR error's style (`schema.rs:200-204`).
- `execute_batch` regression test: `store.append` ordering is deterministic (original action order) even when the underlying `execute_typed_read` futures complete out of order.
- No test or code path allows `ModelRequest.tools` to become non-empty, and `structured_observed_from_tracker`'s `ModelEvent::ToolCall` error path remains reachable and tested.

---

# 9. Phase 4 — Whisker Retriever v2

## 9.1 Goal

Fuse the signals `whisker-context-engine` already computes but never queries (`symbols`, `imports`, §2.3) into `retrieve()`'s ranking, and optionally move chunk *boundaries* toward AST node boundaries for tree-sitter-supported languages, keeping the existing line-window chunker as fallback. Embeddings remain explicitly deferred (§4.2).

## 9.2 Data structures

```rust
// crates/whisker-context-engine/src/lib.rs
pub struct CandidateHit {
    pub path: PathBuf,
    pub chunk_span: (usize, usize),
    pub signals: HitSignals,
    pub score: f64,
}

pub struct HitSignals {
    pub bm25: Option<f64>,             // from chunks FTS5 MATCH, existing
    pub symbol_match: Option<SymbolMatch>,   // new: from the existing `symbols` table
    pub import_proximity: Option<f64>,       // new: from the existing `imports` table
    pub path_bonus: bool,              // existing +5000 filename/path substring signal
    pub changed_file_bonus: bool,      // existing +2500 git-changed signal
    pub git_recency: Option<f64>,      // existing last_commit-derived signal
}

pub struct SymbolMatch {
    pub name: String,
    pub kind: String,   // function_item / struct_item / class_definition / ...
    pub line: usize,
}
```

## 9.3 Functions and files to change

- `crates/whisker-context-engine/src/lib.rs`: `retrieve()` (`lib.rs:929-986`) is restructured into three stages: (1) gather candidates from the existing `chunks` FTS5 query, a new query against the existing `symbols` table (name/kind match against the task query terms), and a new query against the existing `imports` table (proximity to already-selected files); (2) merge/dedupe by `(path, overlapping chunk_span)`, combining `HitSignals` per candidate; (3) rerank by a combined score and truncate by the existing `RetrievalBudget` (`lib.rs:390-397`, `maximum_hits: 12, maximum_bytes: 64 * 1024`) — the budget contract does not change, fusion happens strictly before it.
- `chunks()` (`lib.rs:1635-1651`): optionally gains a tree-sitter-node-boundary mode, gated per supported language, using the parse the `extract_symbols` path (`lib.rs:1663-1730`) already performs — languages without a grammar, or any parse failure, fall back to the existing `CHUNK_LINES=100`/`CHUNK_OVERLAP=10` line window unchanged.
- The Tier1 "widen to whole repo" fallback (`lib.rs:676-686`) is unchanged by this phase — it operates one layer above `retrieve()`.

## 9.4 PawGate / NineLives rollout

None required. Retrieval is read-only context assembly and is never a PawGate-authorized or Claw-executed action — this phase touches none of the safety surface. Stated explicitly so reviewers of this PRD can confirm "context orchestration" work stays scoped away from the authorization system.

## 9.5 Acceptance criteria

- On a regression corpus of symbol-named queries (e.g. "AgentTurn", "validate_turn"), retrieval recall improves measurably versus today's `chunks`-only ranking (today's SQL never joins `symbols`, confirmed `lib.rs:941-953`).
- For tree-sitter-supported languages, chunk boundaries no longer bisect a function/struct body in fixture tests where the old `CHUNK_LINES=100` window did; unsupported languages/parse failures still chunk exactly as today (fallback verified by test).
- `RetrievalBudget` still bounds final output size after fusion in all cases — fusion never bypasses the existing budget contract.
- No vector/embedding dependency is introduced (explicit non-goal check, §4.2).

---

# 10. Phase 5 — Context-Isolated Scout Subagent + Context-as-UI-Primitive

## 10.1 Goal

Add the subagent shape §2.5 confirms does not exist today: an isolated-context caller that performs its own multi-step, read-only exploration (using Phase 3's action-set loop) and returns only compact structured findings, keeping raw exploration output out of the main coding_worker's `conversation_messages`. Then surface Phase 1's context ledger in the IDE composer so the person supervising the agent can see, pin, and understand what is being sent — the composer's only scheduled editor-adjacent work in this PRD (§4.1).

## 10.2 Data structures

```rust
// crates/model-selection/src/lib.rs — ModelRole gains a 7th, real variant
pub enum ModelRole {
    CodingWorker, Planner, Judge, Summarizer, Embedding, FastRouter,
    Scout,   // new — unlike "reviewer"/"utility" (§2.5), this ships with a real
             // model_for("scout") call site, not just a routing label.
}
```

```rust
// crates/agent-runtime/src/agent.rs or a new module beside it
pub struct ScoutRequest {
    pub scout_id: ScoutId,
    pub parent_turn_id: TurnId,       // Phase 1
    pub objective: String,
    pub max_actions: u32,             // bounded, e.g. 8
    pub max_tokens: u64,
    pub allowed_action_kinds: Vec<ActionKind>, // ReadOnly only, enforced at validation
}

pub struct ScoutFinding {
    pub scout_id: ScoutId,
    pub evidence: Vec<EvidenceRef>,
    pub conclusions: Vec<String>,
    pub confidence: ScoutConfidence,
}

pub struct EvidenceRef {
    pub path: PathBuf,
    pub line_range: (u32, u32),
    pub excerpt: String,
}

pub enum ScoutConfidence { High, Medium, Low }
```

Composer-facing types (`crates/purrcode-ide/src/model.rs`):

```rust
pub struct PinnedContextRef {
    pub label: String,
    pub class: ContextClass,      // Phase 1
    pub why_included: WhyIncluded, // Phase 1
    pub estimated_tokens: u64,
}
```

## 10.3 Functions and files to change

- `crates/agent-runtime/src/agent.rs`: new `run_scout(request: ScoutRequest, ...) -> Result<ScoutFinding, AgentError>`, structured the same way as `run_planner()` (`agent.rs:1053-1144`) — its own fresh model conversation, its own `retrieve()` context — but, unlike `run_planner`, it loops over Phase 3's action-set mechanism (bounded by `max_actions`/`max_tokens`) before returning. This multi-step-then-summarize loop does not exist anywhere in the codebase today (`run_planner` issues exactly one `retrieve()` call, §2.5) — it is new, not a repurposing of `JudgedSupervisorWorker` (`purrcode-daemon/src/lib.rs:1051-1256`), which stays reserved for parallel decomposed **write** subtasks with worktree/merge semantics a read-only Scout does not need.
- `crates/agent-runtime/src/context.rs`: new `build_scout_messages(...)`, alongside `build_plan_messages`/`build_contextual_request`, producing Scout's isolated conversation — never touching the caller's `conversation_messages`.
- `crates/model-selection/src/lib.rs`: add `ModelRole::Scout` and its `as_str()` mapping (`"scout"`).
- `crates/purrcode-daemon/src/lib.rs`: dispatch entrypoint for Scout requests, tagging every action Scout proposes with `scout_id` for durable audit.
- `crates/purrcode-ide/src/app/workbench.rs`: rewrite `composer_widget` (`workbench.rs:615-716`) — the static `"/ commands   @ files   # symbols"` hint (`657-663`) becomes real: `@` resolves against `whisker-context-engine`'s existing symbol/path index and inserts a `PinnedContextRef` chip (never raw unvalidated text). Render a chip list above the composer, each chip showing its `ContextClass` label, size, and `WhyIncluded` provenance sourced from Phase 1's ledger. Render a token-usage meter broken down by `ContextClass`, reading `GET /v1/sessions/{id}/context-ledger/{turn_id}` (Phase 1).
- `crates/purrcode-ide/src/model.rs`: add `PinnedContextRef` and chip-rendering types.

## 10.4 PawGate / NineLives rollout

Scout actions are real `ProposedAction`s that go through `Policy::evaluate` and Claw execution exactly like main-loop actions — Scout does not bypass authorization, it runs its own isolated conversation and its own bounded action budget. Every Scout action is durably logged via the same `SessionStore::append` path (NineLives), tagged with `scout_id`, so a Scout's exploration is fully auditable and replayable, not a black box the main agent trusts blindly. Because `allowed_action_kinds` is `ReadOnly`-only and enforced the same way Phase 3 enforces "no mutating action may share a turn" (§8.2), Scout needs no worktree isolation and no merge coordinator, unlike `JudgedSupervisorWorker`.

## 10.5 Composer scope note

This is the only IDE editor-adjacent work scheduled by this PRD (§4.1). It is agent-supervision UI — what the model was shown and why — not a step toward general editor-feature parity.

## 10.6 Acceptance criteria

- A task requiring exploration across ≥5 files returns a `ScoutFinding` whose `EvidenceRef` entries are verifiable (each `line_range` matches real file content at call time) without the main coding_worker's `conversation_messages` ever containing the raw grep/read output.
- Scout's actions appear in the NineLives event log tagged with `scout_id`, independently auditable/replayable like any other session's events.
- A composer `@file` mention inserts a chip referencing a real, validated repository path — never raw text sent as if it were user prose.
- The composer's token-usage meter total equals the value `enforce_budget_before_send` already computes for that turn — one number, not two drifting estimates.

---

# 11. Cross-Cutting Safety Invariants

Restated once, precisely, because every phase above references it:

1. `pawgate-runtime::Policy::evaluate` signature is unchanged across all five phases. It is called once per action, whether that action arrived alone (today, and still for mutations after Phase 3) or as part of a read-only action set (Phase 3, Phase 5's Scout).
2. `claw-sandbox::ToolRuntime::execute` is unchanged. Phase 3 adds `execute_batch` beside it; it does not replace or weaken `execute`'s single-use-authorization, serialized-append contract for mutating actions.
3. `ninelives-recovery::SessionStore::append`/`reduce_event`/`load`/`events` are unchanged. Every new event type introduced by this PRD (`ContextAssembled`, `CheckpointCompacted`) flows through the identical append-only, replayable path as every `SessionEvent` variant that exists today.
4. No phase introduces a second event log, a second authorization path, or a code path that executes a mutating action without an individual PawGate judgment and an individual Claw single-use authorization.
5. `ModelRequest.tools` stays `Vec::new()` everywhere, forever, within this PRD's scope (§4.4). Structured-JSON turns remain the only model-facing contract.

---

# 12. Implementation Plan

## PR0 — Verified Baseline Documentation

- land this PRD;
- no code changes.

## PR1 — Context Ledger Foundations (Phase 1)

- `TurnId`/`SpanId`/`ToolCallId`/`ContextLedgerEntry`/`ContextClass`/`WhyIncluded` in `runtime-core`;
- `SessionEvent::ContextAssembled` + reducer arm;
- `turn_id` threaded through `run_until_pause`/`run_planner`;
- `build_messages()` returns per-section ledger data;
- presentation endpoint `GET /v1/sessions/{id}/context-ledger/{turn_id}`.

## PR2 — IDE Turn/Span Correlation (Phase 1)

- `Message`/`ActivityLine` gain `turn_id`/`span_id`/`parent_span_id`;
- `work_log_anchor` replaced by exact correlation;
- remove the now-stale "no per-turn activity IDs" comment.

## PR3 — Semantic Checkpoint Compaction (Phase 2)

- `SemanticCheckpoint`/`FailedAttempt`/`CheckpointDecision`;
- `SessionEvent::CheckpointCompacted` + additive (non-overwriting) reducer;
- token-aware compaction trigger alongside the existing action-count trigger;
- `conversation_messages` bounded on compaction;
- `build_messages()` renders structured checkpoint fields.

## PR4 — Action-Set Schema and Validation (Phase 3)

- `AgentTurn.action` → `AgentTurn.actions: Vec<AgentAction>`;
- `validate_turn` rewrite (single-mutation / multi-read rule);
- `ActionClass`/`is_read_only` classification;
- prompt text update in `build_messages()`.

## PR5 — Claw Batch Execution (Phase 3)

- `ToolRuntime::execute_batch`;
- concurrent `execute_typed_read`, serialized `store.append`;
- `run_until_pause` wiring for read-only action sets.

## PR6 — Whisker Retriever v2 (Phase 4)

- `CandidateHit`/`HitSignals`/`SymbolMatch`;
- `retrieve()` restructured into gather/fuse/budget stages;
- symbol and import table queries;
- optional AST-node-boundary chunking with line-window fallback.

## PR7 — Scout Subagent (Phase 5)

- `ModelRole::Scout` + `model_for("scout")` call site;
- `ScoutRequest`/`ScoutFinding`/`EvidenceRef`;
- `run_scout()` + `build_scout_messages()`;
- daemon dispatch with `scout_id`-tagged durable logging.

## PR8 — Composer Context Primitives (Phase 5)

- `@file`/`@symbol` resolution against the existing whisker index;
- `PinnedContextRef` chips with provenance;
- token-usage-by-class meter reading the Phase 1 ledger endpoint.

## PR9 — Qualification

- unit tests per phase acceptance criteria (§6.5, §7.5, §8.6, §9.5, §10.6);
- benchmark suite (§13);
- dogfood on a representative multi-file task set;
- regression test confirming `ModelRequest.tools` never becomes non-empty and `structured_observed_from_tracker`'s tool-call error path stays reachable.

---

# 13. Benchmark and Acceptance Criteria

"Context management actually improved" is defined measurably, not impressionistically. All metrics below are computed from the Phase 1 context ledger and the existing NineLives event log — no new telemetry system is introduced.

```text
Tool round-trips per task
  = count of distinct turn_ids that produced at least one ActionProposed,
    for a fixed benchmark task set (read-heavy exploration tasks weighted).
  Target: materially lower than today's 1 round-trip per action baseline
  for tasks whose reference solution requires ≥3 sequential reads.

Tokens per task
  = sum of ContextLedgerEntry.total_estimated_tokens across all turns in a
    session, for the same fixed benchmark task set.
  Target: no regression versus v1.0 baseline; improvement expected once
  Phase 2's checkpoint replaces resending the full unbounded
  conversation_messages tail.

Repeat-read rate
  = fraction of ReadOnly actions in a session whose (path, overlapping
    line range) was already read earlier in the same session, computed
    by replaying the NineLives event log.
  Target: trends toward zero after Phase 2 (failed_attempts/files_inspected
  persist across compaction) and Phase 5 (Scout returns findings instead
  of raw reads the main loop would otherwise re-read).

Checkpoint recall of failed_attempts
  = percent of FailedAttempt entries present in a checkpoint immediately
    before compaction N that are still present (via the
    superseded_checkpoint_id chain) in the checkpoint used to build the
    prompt at compaction N+2.
  Target: 100%. Today this is 0% beyond the 6 actions
  RETAINED_ACTIONS_AFTER_COMPACTION retains, because context_summary is
  overwritten wholesale (§2.2) — this is the single sharpest before/after
  number for Phase 2.

Context-selection ratio (Whisker v2)
  = selected context tokens / eligible candidate context tokens,
    computed post-fusion, pre-RetrievalBudget-truncation.
  Reported per task category (symbol-lookup-heavy vs prose-search-heavy)
  to show whether fusion changed what gets selected, not just how much.

IDE Work Log correlation accuracy
  = percent of rendered ActivityLine entries whose turn_id exactly matches
    the turn that produced them (post Phase 1), vs. today's positional
    work_log_anchor heuristic, which has no such guarantee.
  Target: 100%, by construction, once Phase 1 ships.
```

Do not report any of these as a "token savings" or "faster" claim without the fixed benchmark task set and the reproducible before/after comparison this section defines — matching this codebase's own existing efficiency-reporting standard from the v1.0 PRD.

---

# 14. Required Testing

## 14.1 Phase 1

- `ContextLedgerEntry` section sum equals `enforce_budget_before_send`'s aggregate count for the same turn.
- NineLives replay reconstructs identical ledger entries.
- IDE `Message`/`ActivityLine` `turn_id` correlation, no positional fallback remaining.

## 14.2 Phase 2

- Two-compaction `failed_attempts` survival (§7.5) — the primary regression test for this PRD.
- Token-threshold-triggered compaction independent of action count.
- `conversation_messages` bounded post-compaction.

## 14.3 Phase 3

- `validate_turn` accepts a read-only action set, rejects a mixed mutating+other-action turn.
- `execute_batch` deterministic append ordering under out-of-order I/O completion.
- Every action in a batch produces its own `ActionProposed`/`JudgmentRecorded` pair.
- `ModelRequest.tools` stays empty; `ModelEvent::ToolCall` still errors.

## 14.4 Phase 4

- Symbol-query recall improvement on a fixed corpus.
- Chunk-boundary fixture tests for tree-sitter-supported languages, with explicit fallback tests for unsupported/unparseable input.
- `RetrievalBudget` still bounds fused output.

## 14.5 Phase 5

- Scout evidence verifiability against real file content.
- Scout actions durably logged and replayable with `scout_id`.
- Composer `@file` chip resolves only to validated repository paths.
- Composer token meter matches `enforce_budget_before_send`'s value exactly.

## 14.6 Cross-cutting

- Full existing PawGate/Claw/NineLives test suites pass unmodified in behavior (only additive coverage for `execute_batch`/`CheckpointCompacted`/`ContextAssembled`).
- No new authorization bypass path exists for any action introduced by this PRD (Scout actions, batched read actions) — verified by the same authorization test harness used for v1.0's Security Requirements.
