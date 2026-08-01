# PurrCode v1.0 Master Product Requirements Document

## IDE-Grade Local Coding Agent, TUI-First Runtime, GitHub-Native Delivery

**Document status:** Ready for Codex implementation  
**Target release:** `v1.0.0`  
**Baseline release:** `v0.9.0`  
**Baseline merge commit:** `cd8c4da039af26573685146dcd1f9dd46f98bae1`  
**Primary runtime interface:** Native TUI  
**Primary visual development interface:** VS Code-compatible IDE companion  
**Automation interface:** CLI  
**Source-control integration:** GitHub  
**Studio:** Maintenance-only  
**Web product:** Out of scope  
**Mobile product:** Out of scope  

---

# 0. Codex Master Goal

Implement PurrCode v1.0 as a polished, coherent coding-agent product whose interaction quality matches the supplied IDE reference while preserving the proven v0.9 runtime.

Use these supplied assets as authoritative references:

```text
assets/purrcode-v1-ide-reference.png
assets/purrcode-logo-source.png
```

The target experience is:

```text
Open repository
→ open PurrCode
→ confirm or select a model
→ describe the desired outcome
→ PurrCode inspects, plans, edits, executes, tests, repairs, and validates
→ review changes inside the IDE
→ commit, push, and create a GitHub pull request
```

The user must experience one product across TUI and IDE.

Do not create a second agent runtime, second session model, second permission system, second terminal implementation, or second validation path.

PurrCode v1.0 is primarily a **usability, adaptive orchestration, IDE integration, GitHub completion, branding, efficiency, and product-qualification release**.

The runtime must support a smooth adaptive execution system:

```text
Direct workflow
Standard workflow
PurrCode Ultra workflow
```

It must also support:

- multiple securely stored provider credentials;
- deterministic task-aware model routing;
- user-controlled online-search policy;
- governed MCP tools;
- uncertainty-triggered research;
- explicit token and cost budgets;
- measurable context and token efficiency.

---

# 1. Baseline Rule

PurrCode v0.9 already implements significant runtime and interface capability.

The following are compatibility invariants and must not be reimplemented from scratch:

- bare `purrcode` launches the TUI;
- TUI-first product entry;
- provider/model onboarding inside the TUI;
- selectable models;
- Ask / Plan / Build / Review task modes;
- Ask / Auto / Full Access permission modes;
- real PTY/ConPTY terminal runtime;
- incremental terminal output;
- terminal ownership transfer;
- semantic activity APIs;
- validation summary APIs;
- durable session summaries;
- automatic build/test orchestration;
- bounded repair;
- Plan feedback and plan revision;
- Plan-to-Build continuation in the same session;
- isolated worktrees;
- PawGate authorization;
- Claw execution;
- NineLives recovery;
- evidence-backed completion;
- optional session-first Studio.

Codex must extend these capabilities through stable contracts.

Codex must not replace them with UI-only mock implementations.

---

# 2. Product Vision

PurrCode is:

> A local-first autonomous coding agent that is fast in the terminal, visual and productive inside the IDE, and capable of completing the engineering workflow through GitHub.

The product must feel:

- calm when idle;
- immediately understandable to a new developer;
- powerful without exposing runtime internals;
- visually coherent;
- responsive;
- IDE-native;
- trustworthy;
- evidence-based;
- easy to control;
- capable of finishing work without repetitive prompting.

PurrCode is not primarily:

- a runtime dashboard;
- an event-log viewer;
- an Azure console;
- an agent marketplace;
- a browser administration application;
- a full custom IDE fork;
- a mobile coding application;
- a generic workflow builder.

---

# 3. v1.0 Product Outcomes

v1.0 must deliver:

1. A polished TUI that remains the default runtime interface.
2. A production-quality VS Code-compatible PurrCode Workbench.
3. One durable session shared between TUI and IDE.
4. A one-action TUI-to-IDE handoff.
5. A one-action IDE-to-TUI handoff.
6. A consistent visual and interaction language.
7. A clear model and provider experience.
8. A clear permission experience.
9. A clear Plan / Build / Review lifecycle.
10. Plan, changes, tests, and results as first-class artifacts.
11. IDE-native code, diff, diagnostics, tests, and terminal integration.
12. One-time GitHub connection.
13. GitHub issue, branch, commit, push, pull request, and check workflows.
14. Executable user-local environment provisioning for the highest-priority toolchains.
15. Automatic build, test, repair, and final validation.
16. Durable resume and recovery.
17. Production branding based on the supplied ragdoll-cat logo.
18. New-user first useful task in under five minutes.
19. Real dogfood qualification.
20. No unfinished primary navigation or placeholder feature.
21. Adaptive Direct / Standard / Ultra workflow selection.
22. Bounded parallel specialist workflows for complex tasks.
23. Multiple secure API-key and provider profiles.
24. Task-aware model, provider, and credential routing.
25. Search Off / Auto / Always policy.
26. Governed MCP discovery and execution.
27. Evidence-triggered online research when local information is insufficient.
28. Per-request, per-session, per-model, per-provider, and per-key usage accounting.
29. Token-aware context assembly and measurable efficiency.
30. User-configurable token, cost, search, and workflow budgets.

---

# 4. Non-Goals

The following are not v1.0 release gates:

- full browser Web product;
- mobile application;
- Azure resource management;
- Azure deployment dashboard;
- multi-tenant organization administration;
- Agent Factory dashboard;
- marketplace;
- GitLab;
- JetBrains plugin;
- custom Code OSS fork;
- multi-agent topology visualization;
- automatic merge;
- complex reviewer assignment;
- organization-wide policy administration;
- unrestricted operating-system package management.

Existing experimental code may remain, but it must not control the primary experience.

---

# 5. Reference Asset Authority

## 5.1 IDE reference

`assets/purrcode-v1-ide-reference.png` is the visual north star.

It defines the intended qualities:

- dark, focused IDE environment;
- slim application chrome;
- compact controls;
- clear active-session state;
- useful project and session navigation;
- agent conversation beside code;
- plan/checklist progress;
- editor-first code review;
- contextual diff, tests, terminal, problems, and output;
- visible model and permission state;
- minimal unnecessary whitespace;
- no dashboard cards;
- no internal runtime noise.

The implementation does not need to reproduce every pixel.

It must reproduce the interaction hierarchy and practical density.

## 5.2 Logo reference

`assets/purrcode-logo-source.png` is the authoritative mascot and wordmark source.

Do not regenerate or replace the mascot.

Derive production assets from this source while preserving:

- ragdoll-cat face;
- cream and gray-brown fur;
- large blue eyes;
- purple collar;
- round tag;
- friendly but technical tone;
- white `Purr` and blue `Code` wordmark.

Production cleanup may remove excess transparent canvas, raster artifacts, and small-scale detail that does not survive icon sizes.

---

# 6. Product Surfaces

## 6.1 Native TUI

Command:

```bash
purrcode
```

The TUI remains the default product entry.

It is optimized for:

- rapid task entry;
- SSH;
- Linux VM use;
- local terminal users;
- autonomous progress monitoring;
- approvals;
- terminal access;
- recovery;
- fast model changes.

## 6.2 IDE companion

Commands:

```bash
purrcode ide
purrcode ide --session <session-id>
```

TUI actions:

```text
/ide
Ctrl+Shift+I
```

The IDE companion is implemented as a VS Code-compatible extension in v1.0.

It is optimized for:

- code navigation;
- selected-code context;
- file review;
- native diff;
- diagnostics;
- tests;
- terminal;
- plan/spec editing;
- GitHub delivery.

It is not a second runtime.

## 6.3 CLI

The CLI remains available for:

```bash
purrcode run "<objective>"
purrcode plan "<objective>"
purrcode ci "<objective>"
purrcode resume
purrcode doctor
```

## 6.4 Studio

Studio remains maintenance-only.

Requirements:

- preserve `purrcode studio`;
- preserve `purrcode ui`;
- preserve `/studio`;
- preserve current secure authentication;
- preserve session compatibility;
- fix P0 regressions;
- do not add major new Studio surfaces;
- do not remove Studio while implementing the IDE.

---

# 7. Unified Product Model

All clients must use one authoritative state.

```rust
pub struct UnifiedSession {
    pub session_id: SessionId,
    pub repository: RepositoryPresentation,
    pub objective: String,
    pub title: String,
    pub conversation: Vec<MessageRef>,
    pub task_mode: TaskMode,
    pub execution_style: ExecutionStyle,
    pub selected_model: ModelSelection,
    pub permission_grant: PermissionGrantRef,
    pub current_deliverable: Option<ArtifactRef>,
    pub activity: Vec<ActivityRef>,
    pub terminals: Vec<TerminalRef>,
    pub changes: ChangeSummary,
    pub validation: ValidationSummary,
    pub github: Option<GitHubSessionState>,
    pub status: SessionStatus,
}
```

No client-specific execution state may affect runtime behavior.

TUI, IDE, CLI, and Studio must agree on:

- session;
- model;
- task mode;
- permission;
- objective;
- current artifact;
- terminal state;
- validation state;
- GitHub state;
- completion state.

---

# 8. User-Facing Modes

## 8.1 Task modes

```text
Ask
Plan
Build
Review
```

### Ask

Read-only repository understanding.

### Plan

Create and revise a repository-aware implementation plan without writing files.

### Build

Inspect, edit, execute, test, repair, and validate.

### Review

Review code, diff, risk, and validation.

## 8.2 Execution styles

```text
Collaborative
Autonomous
```

### Collaborative

The user can guide each major stage.

### Autonomous

PurrCode continues until completion or a genuine blocker.

## 8.3 Permission modes

```text
Ask
Auto
Full Access
```

### Ask

Request permission before writes, execution, installation, network, and external effects.

### Auto

Permit normal repository edits and recognized engineering commands. Ask for unexpected or risky effects.

### Full Access

Permit all capabilities already available to the PurrCode process and connected identities.

Full Access does not create new OS, network, GitHub, or cloud permissions.

The model may never change permission mode.

---


# 9. Adaptive Workflow Orchestration

## 9.1 Product goal

PurrCode must choose the smallest workflow capable of completing the task safely and reliably.

The user must not need to manually design an agent graph.

The runtime chooses between:

```text
Direct
Standard
Ultra
```

The user may override the choice.

PurrCode Ultra is PurrCode's product name for a bounded multi-workflow execution mode. It must not be described as an official Claude Code feature name.

## 9.2 Workflow profiles

### Direct

Use for:

- questions;
- one-file fixes;
- obvious local changes;
- small tests;
- deterministic formatting or configuration changes.

Default shape:

```text
One primary coding workflow
→ focused validation
→ completion
```

Default search policy:

```text
Off
```

unless the user explicitly requests online research or the task inherently requires current external information.

### Standard

Use for:

- normal feature work;
- moderate refactors;
- changes across a small number of modules;
- dependency updates with known local evidence;
- tasks requiring implementation plus independent validation.

Default shape:

```text
Coordinator
├── implementation workflow
└── validation/review workflow
```

Workflows may be sequential or partially parallel.

Default search policy:

```text
Auto
```

but search occurs only when an evidence-based trigger is present.

### Ultra

Use for:

- migrations;
- unfamiliar repositories;
- cross-layer features;
- multi-module refactors;
- complex debugging;
- tasks with independent research, implementation, test, and review work;
- tasks where parallel bounded analysis materially reduces wall time.

Default shape:

```text
Coordinator
├── repository-analysis workflow
├── research/documentation workflow, when allowed
├── implementation workflow A
├── implementation workflow B, when file scopes do not overlap
├── validation workflow
└── independent review workflow
```

Ultra must remain bounded.

Default limits:

```text
Maximum active specialist workflows: 5
Maximum workflow depth: 2
Maximum automatic repair generations: configurable
Maximum parallel writers to the same file scope: 1
```

## 9.3 Task complexity classification

Define:

```rust
pub enum TaskComplexity {
    Simple,
    Moderate,
    Complex,
    Unknown,
}
```

Classification must use evidence such as:

- number of repository modules affected;
- number of candidate files;
- language/build-system count;
- requested artifact count;
- migration or compatibility requirements;
- external API or documentation dependency;
- uncertainty from repository inspection;
- baseline test state;
- expected validation stages;
- number of independent deliverables;
- user-selected quality and budget profile.

Do not classify only from prompt length.

Do not use model self-reported confidence as the sole trigger.

The classification decision must be durable and explainable.

```rust
pub struct ComplexityDecision {
    pub complexity: TaskComplexity,
    pub evidence: Vec<ComplexitySignal>,
    pub selected_workflow: WorkflowProfile,
    pub selected_search_policy: SearchPolicy,
    pub selected_budget: BudgetProfile,
}
```

## 9.4 Workflow plan contract

```rust
pub struct WorkflowPlan {
    pub plan_id: WorkflowPlanId,
    pub profile: WorkflowProfile,
    pub objective: String,
    pub lanes: Vec<WorkflowLane>,
    pub dependencies: Vec<WorkflowDependency>,
    pub budgets: WorkflowBudgets,
    pub search_policy: SearchPolicy,
    pub completion_condition: CompletionCondition,
}
```

Lane types:

```rust
pub enum WorkflowLaneKind {
    RepositoryAnalysis,
    Planning,
    Research,
    Implementation,
    Validation,
    Review,
    GitHubDelivery,
    Recovery,
}
```

Each lane must declare:

- objective;
- allowed tools;
- read scope;
- write scope;
- model route;
- token budget;
- wall-time budget;
- expected evidence;
- completion condition;
- dependency IDs.

## 9.5 Parallel-write safety

Ultra must not become uncontrolled multi-agent editing.

Rules:

1. Only one active writer may own a file scope.
2. Other workflows may read that scope.
3. Parallel writers must use disjoint file scopes or isolated worktrees.
4. A merge coordinator must inspect every cross-workflow effect.
5. Conflicts must be surfaced explicitly.
6. Validation must run after merged effects.
7. A workflow may not directly apply another workflow's unverified patch.
8. Duplicate external effects must remain impossible.

## 9.6 Workflow communication

Workflows communicate through typed artifacts and bounded summaries.

Do not copy full conversations into every workflow.

Shared artifacts may include:

- repository map;
- task plan;
- symbol list;
- API contract;
- research findings;
- patch summary;
- failure summary;
- validation evidence.

Each artifact must include:

- source;
- timestamp;
- scope;
- size;
- trust classification;
- digest.

## 9.7 User controls

Expose:

```text
Workflow: Auto
Workflow: Direct
Workflow: Standard
Workflow: Ultra
```

TUI and IDE commands:

```text
/workflow auto
/workflow direct
/workflow standard
/workflow ultra
```

CLI:

```bash
purrcode run "<objective>" --workflow auto
purrcode run "<objective>" --workflow direct
purrcode run "<objective>" --workflow ultra
```

The default is:

```text
Auto
```

The user may force Direct to prohibit workflow fan-out.

The user may force Ultra when quality or parallel exploration is more important than cost.

## 9.8 UI presentation

The default UI must not show an agent-swarm dashboard.

Show one semantic task checklist.

Ultra details appear in a contextual view:

```text
Ultra workflow

✓ Repository analysis
● Backend implementation
● Frontend implementation
○ Validation
○ Independent review
```

The user may inspect:

- workflow objective;
- selected model;
- token budget;
- current status;
- affected scope;
- evidence.

---

# 10. Multiple Provider Credentials and Adaptive Routing

## 10.1 Product goal

A user may configure multiple:

- providers;
- models;
- API keys;
- accounts;
- enterprise gateways;
- local model endpoints.

PurrCode must route requests according to task requirements, user policy, provider health, privacy, budget, and model qualification.

The user must be able to prohibit automatic routing.

## 10.2 Credential security

Every credential must be stored as a secure reference.

```rust
pub struct CredentialProfile {
    pub credential_id: CredentialId,
    pub provider_id: ProviderId,
    pub label: String,
    pub secret_reference: SecretReference,
    pub allowed_models: Vec<ModelPattern>,
    pub priority: u16,
    pub enabled: bool,
    pub budget: CredentialBudget,
}
```

Never store raw keys in:

- TOML;
- repository files;
- model context;
- session events;
- browser storage;
- terminal commands;
- child-process environments;
- logs;
- evidence bundles.

Supported secure stores:

- operating-system keychain;
- VS Code SecretStorage for extension-owned references;
- approved enterprise secret helper;
- environment reference, when explicitly selected.

## 10.3 Credential pools

A provider may contain multiple credential profiles.

```rust
pub struct CredentialPool {
    pub provider_id: ProviderId,
    pub credentials: Vec<CredentialProfile>,
    pub strategy: CredentialSelectionStrategy,
}
```

Strategies:

```rust
pub enum CredentialSelectionStrategy {
    Fixed(CredentialId),
    Priority,
    Weighted,
    LowestObservedCost,
    HighestRemainingBudget,
    HealthAware,
}
```

Routing must respect provider terms, rate limits, and configured budgets.

It must not be described or implemented as a method to evade access controls or provider restrictions.

## 10.4 Route decision

```rust
pub struct ModelRouteDecision {
    pub decision_id: RouteDecisionId,
    pub workflow_lane_id: WorkflowLaneId,
    pub provider_id: ProviderId,
    pub model_id: ModelId,
    pub credential_id: CredentialId,
    pub reasons: Vec<RouteReason>,
    pub expected_capabilities: ModelCapabilities,
    pub privacy_class: PrivacyClass,
    pub budget_snapshot: BudgetSnapshot,
}
```

Selection evidence may include:

- required tool calling;
- structured-output qualification;
- coding qualification;
- context capacity;
- task complexity;
- expected latency;
- remaining budget;
- observed provider health;
- rate-limit state;
- privacy restrictions;
- user model pin;
- local resource capacity.

## 10.5 Routing profiles

Expose:

```text
Model routing: Fixed
Model routing: Auto
Model routing: Economy
Model routing: Quality
```

### Fixed

Use the selected model and credential only.

No automatic provider fallback.

### Auto

Choose the smallest qualified route that satisfies the task.

### Economy

Prioritize lower expected token cost while preserving required capabilities.

### Quality

Prioritize the strongest qualified route inside the user's budget.

## 10.6 Fallback policy

Fallback must be explicit.

Allowed fallback conditions:

- provider unavailable;
- rate limit;
- transient authentication failure after refresh;
- model unavailable;
- context limit;
- qualification mismatch.

A route must not silently cross:

- local to remote;
- approved provider to unapproved provider;
- private endpoint to public endpoint;
- user-pinned model to another model

unless the configured fallback policy permits it.

Show:

```text
GPT-5 unavailable
Fell back to qwen2.5-coder:7b under Auto routing
```

## 10.7 Multi-model Ultra routing

Ultra may assign different qualified models to different lanes.

Example:

```text
Repository analysis: local coding model
Research: fast remote model
Implementation: strongest coding model
Validation review: independent judge model
Summarization: small utility model
```

This is optional.

The default should reuse one selected model when that is sufficient.

Do not multiply model calls merely because multiple models are configured.

---

# 11. Search Policy, Online Research, and MCP

## 11.1 Search policy

Define:

```rust
pub enum SearchPolicy {
    Off,
    Auto,
    Always,
}
```

### Off

No online search.

No network MCP tool may be used for research.

Local repository, local documentation, configured offline resources, and local tools remain available.

The user instruction:

```text
Do not search online
```

must produce SearchPolicy::Off and must be enforced.

### Auto

Search only when an evidence-based trigger exists.

### Always

Search may be used proactively where it can improve correctness.

Always does not mean unbounded searching.

## 11.2 Default policy by workflow

```text
Direct:   Off
Standard: Auto
Ultra:    Auto
```

The user setting overrides the default.

## 11.3 Auto-search triggers

Search may be triggered when:

- the user explicitly requests current or external information;
- a dependency/API version is absent from repository evidence;
- current documentation is required;
- an error references an unknown external package behavior;
- repeated validation failure indicates external compatibility uncertainty;
- a security advisory or breaking change must be verified;
- an external issue, pull request, or specification is part of the task;
- local docs conflict;
- a required fact cannot be verified locally.

Search must not be triggered merely because:

- a task is long;
- the model says it is unsure;
- more context might be interesting;
- a simple local edit can be completed and validated without external information.

## 11.4 Research decision

```rust
pub struct ResearchDecision {
    pub search_policy: SearchPolicy,
    pub trigger: Option<ResearchTrigger>,
    pub local_evidence_checked: Vec<EvidenceRef>,
    pub query_budget: u32,
    pub token_budget: u64,
    pub allowed_sources: SourcePolicy,
}
```

The decision must be visible in technical details.

## 11.5 Source policy

For technical questions, prefer:

1. official documentation;
2. primary source repositories;
3. standards;
4. release notes;
5. research papers;
6. high-quality secondary sources.

Search findings are untrusted input.

Do not execute code copied from search results without normal inspection and authorization.

Research artifacts must preserve source attribution.

## 11.6 MCP support

MCP is a governed tool integration layer.

Support:

- local stdio MCP servers;
- remote HTTP MCP servers;
- remote SSE MCP servers where required;
- OAuth-authenticated MCP;
- project scope;
- user scope;
- repository scope.

Create:

```rust
pub struct McpServerProfile {
    pub server_id: McpServerId,
    pub name: String,
    pub transport: McpTransport,
    pub scope: McpScope,
    pub capabilities: Vec<McpCapability>,
    pub credential_reference: Option<SecretReference>,
    pub trust: McpTrustClass,
    pub enabled: bool,
}
```

## 11.7 MCP authorization

Every MCP tool call must:

- be represented as a typed action;
- pass PawGate;
- respect permission mode;
- respect SearchPolicy;
- have bounded input/output;
- redact secrets;
- record effects;
- declare network scope;
- declare external write effects.

A read-only documentation MCP tool is not equivalent to an external write tool.

## 11.8 MCP output limits

Every server and tool must have:

```text
Maximum response bytes
Maximum response tokens
Maximum records
Timeout
Retry limit
```

Oversized responses must be:

- truncated with explicit notice;
- summarized through a bounded utility workflow;
- stored as an artifact only when authorized.

Never silently inject an unbounded MCP response into every workflow context.

## 11.9 MCP UX

Commands:

```text
/mcp
/mcp add
/mcp enable
/mcp disable
/mcp inspect
```

Settings must show:

- server;
- scope;
- connection;
- trust;
- available tools;
- credential state;
- recent use.

The default task composer may show:

```text
Search: Auto
```

Do not show every MCP server as a permanent top-level control.

---

# 12. Token, Context, and Cost Efficiency

## 12.1 Product goal

PurrCode must measure real model and tool usage and reduce avoidable token consumption without lowering correctness.

It must never claim token savings without a defined measurement basis.

## 12.2 Usage ledger

Record:

```rust
pub struct UsageRecord {
    pub request_id: RequestId,
    pub session_id: SessionId,
    pub workflow_lane_id: Option<WorkflowLaneId>,
    pub provider_id: ProviderId,
    pub model_id: ModelId,
    pub credential_id: CredentialId,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
    pub tool_result_tokens: u64,
    pub search_requests: u32,
    pub mcp_calls: u32,
    pub estimated_cost: Option<Decimal>,
    pub latency_ms: u64,
    pub recorded_at: DateTime<Utc>,
}
```

Aggregate by:

- request;
- lane;
- session;
- repository;
- model;
- provider;
- credential;
- day;
- workflow profile.

## 12.3 Budget profiles

Expose:

```text
Budget: Economy
Budget: Balanced
Budget: Max Quality
Budget: Custom
```

A custom budget may define:

```rust
pub struct BudgetProfile {
    pub maximum_input_tokens: Option<u64>,
    pub maximum_output_tokens: Option<u64>,
    pub maximum_total_tokens: Option<u64>,
    pub maximum_estimated_cost: Option<Decimal>,
    pub maximum_model_calls: Option<u32>,
    pub maximum_search_requests: Option<u32>,
    pub maximum_mcp_calls: Option<u32>,
    pub maximum_wall_time_seconds: Option<u64>,
}
```

Budget exhaustion must be truthful.

Do not silently continue on another key to bypass a user budget.

## 12.4 Token-aware context assembly

Use progressive context.

Order:

```text
Task instruction
→ repository summary
→ directly referenced files/symbols
→ dependency and call-graph neighbors
→ changed files
→ relevant validation failures
→ selected external research
```

Do not send the complete repository unless explicitly justified.

Required techniques:

- symbol-aware retrieval;
- filename and path retrieval;
- semantic retrieval;
- dependency graph expansion;
- changed-file priority;
- deduplication;
- bounded snippets;
- incremental context;
- reusable repository summaries;
- lane-specific context;
- delta context between turns;
- bounded terminal output;
- bounded MCP output;
- bounded search excerpts.

## 12.5 Summary lifecycle

Summaries must be:

- evidence-linked;
- versioned;
- invalidated when source files change;
- scoped to repository revision;
- bounded;
- marked as derived context.

Do not repeatedly summarize unchanged content.

## 12.6 Context caching

When a provider supports prompt/context caching:

- use it through provider contracts;
- record cache read/write tokens;
- isolate caches by repository/session/privacy scope;
- never assume cache support;
- never report cache savings without provider usage evidence.

## 12.7 Model-size routing

Simple tasks should normally use the smallest qualified model.

Complex planning, migration, or unresolved repair may escalate.

Example policy:

```text
Direct → selected model or qualified economy model
Standard → selected coding model
Ultra analysis → fast qualified model
Ultra implementation → strong coding model
Ultra review → independent qualified model when budget permits
```

Escalation must be explainable.

## 12.8 Tool-result compression

Before adding terminal, search, or MCP output to context:

1. preserve raw output as evidence;
2. parse structured diagnostics where possible;
3. extract failures;
4. remove duplicate lines;
5. bound repeated stack traces;
6. provide a digest-linked summary.

The model receives the smallest sufficient representation.

## 12.9 Efficiency metrics

Report:

```text
Total tokens
Input tokens
Output tokens
Cache read/write
Search requests
MCP calls
Estimated model cost
Tokens per validated change
Tokens per passed validation stage
Context-selection ratio
Retry token share
```

Define:

```text
Context-selection ratio =
selected context tokens / eligible candidate context tokens
```

Do not label it "token savings" unless compared against a reproducible baseline.

## 12.10 Token regression suite

Create representative cases.

Fail CI when, without an approved reason:

- a Direct task uses Ultra;
- a local one-file task performs online search;
- unchanged repository context is resent excessively;
- MCP output enters context unbounded;
- retry token share increases beyond the threshold;
- token use grows materially with no validation improvement.

## 12.11 User-facing usage UX

Default UI remains uncluttered.

Show a compact completion summary:

```text
Usage
42K tokens · 6 model calls · no web search
```

Expanded details show:

- route decisions;
- per-workflow usage;
- search;
- MCP;
- cache;
- estimated cost;
- budget remaining.

Commands:

```text
/usage
/budget
/search
/workflow
```

CLI:

```bash
purrcode usage --session <id>
purrcode run "<objective>" --budget economy
purrcode run "<objective>" --search off
purrcode run "<objective>" --max-tokens 100000
```


# 13. Canonical Product States

The following labels are authoritative:

```text
Ready
Thinking
Plan ready
Working
Running command
Testing
Repairing
Permission required
Ready for review
Completed
Failed
Cancelled
Needs recovery
```

Do not show normal users:

```text
paused blocked
plan-only session
SessionPaused
ModelRequestStarted
WorktreeCreated
CheckpointCreated
JudgmentRecorded
```

Every canonical state must define:

- label;
- glyph/icon;
- semantic color;
- primary action;
- secondary action;
- whether input is accepted;
- whether execution is active.

Example:

```text
Plan ready
Primary action: Build this plan
Secondary action: Revise plan
Input: accepted as plan feedback
Execution: paused safely
```

---

# 14. Information Hierarchy

The primary hierarchy is:

```text
1. Current objective and conversation
2. Current deliverable or decision
3. Semantic progress
4. Technical detail
```

Rules:

- Do not repeat the same objective in header, session card, page title, and message body.
- Plans are artifact cards, not activity-log lines.
- Test results are validation cards, not raw terminal summaries.
- Changed files are review cards, not raw event names.
- Runtime preparation steps are collapsed by default.
- Technical evidence is explicitly opened.
- Activity should normally occupy no more than 25% of the conversation area.
- The composer remains visible.
- Empty or unavailable controls must not dominate the screen.

---

# 15. IDE Architecture Strategy

The IDE experience must use native VS Code capabilities rather than recreating an IDE inside a webview.

Use:

- VS Code Activity Bar;
- Explorer;
- native editor tabs;
- native diff editor;
- Problems API;
- Test API where practical;
- integrated terminal;
- Secondary Side Bar;
- commands;
- URI handler;
- secure extension secrets.

Build a custom PurrCode Workbench view only for:

- session conversation;
- artifact cards;
- semantic activity;
- composer;
- model/mode/permission controls;
- approval cards;
- GitHub actions.

Do not build:

- a custom source editor;
- a custom full file explorer;
- a custom diff renderer when native diff is available;
- a custom terminal when the integrated terminal can attach safely;
- a separate session database inside the extension.

---

# 16. IDE Layout Specification

The supplied reference should be implemented through native IDE regions.

## 12.1 Application bar

Show:

```text
PurrCode
Task mode
Execution style
Repository
Branch
Model
Permission
Connection
```

Controls must be compact.

Do not show complete provider endpoint, session UUID, full worktree path, or raw API version.

## 12.2 Activity Bar

Use the monochrome cat-head icon.

Primary PurrCode views:

- Workbench;
- Sessions;
- Plans/Specs;
- GitHub.

Do not add duplicate icons for every runtime subsystem.

## 12.3 Primary Side Bar

Default PurrCode panel:

```text
New session
Search sessions
Recent sessions
Current project
```

A session row shows only:

- short title;
- status;
- relative time;
- optional unread/attention indicator.

Do not show full repository paths in every row.

## 12.4 PurrCode Workbench

The Workbench is conversation-first.

Required areas:

```text
Session title
Current state
Conversation
Current artifact card
Semantic activity
Composer
```

Composer controls:

```text
Add context
Task mode
Execution style
Model
Permission
Send / Stop
```

## 12.5 Editor area

Use native editor tabs for:

- code;
- plan;
- requirements;
- design;
- task list;
- diff;
- generated report.

## 12.6 Bottom panel

Use standard tabs:

```text
DIFF
TESTS
TERMINAL
PROBLEMS
OUTPUT
```

The active tab should be selected based on explicit user action or a meaningful event.

Do not constantly steal focus.

## 12.7 Secondary Side Bar

Use for contextual inspection:

- tests;
- changed files;
- artifact outline;
- GitHub pull request;
- terminal process list.

It may be closed by default.

---

# 17. IDE Workbench Components

## 13.1 User message

Show:

- user label;
- timestamp;
- content;
- attached context summary.

## 13.2 PurrCode response

Show:

- compact mascot glyph;
- concise explanation;
- current step checklist;
- expandable technical detail.

## 13.3 Plan artifact card

Example:

```text
Plan ready

Document search feature
7 implementation steps

✓ Analyze repository
✓ Design search architecture
○ Implement search API
○ Add frontend search UI
○ Add tests
○ Update documentation

[Build this plan] [Revise] [Open as document]
```

## 13.4 Execution card

Example:

```text
Implement search API

Running: python backend/worker/index_documents.py
842 / 2314 documents
36%

[Open terminal] [Stop]
```

## 13.5 Test card

Example:

```text
Tests

7 passed
1.2 seconds

[Open tests] [Open terminal]
```

## 13.6 Changes card

Example:

```text
Modified 3 files

backend/api/search.py
backend/worker/index.py
frontend/src/components/SearchBox.tsx

[Review changes]
```

## 13.7 Approval card

Example:

```text
Permission required

Install declared project dependencies
Command: npm install
Scope: current repository
Network: package registry

[Approve once] [Allow for this run] [Reject]
```

## 13.8 Completion card

Example:

```text
Ready for review

3 files changed
7 tests passed
No required validation is missing

[Review changes] [Commit] [Create pull request]
```

---

# 18. TUI Experience Specification

The TUI must share the same information hierarchy and vocabulary.

Persistent regions:

```text
Header
Conversation
Composer
```

Contextual regions:

```text
Plan
Changes
Tests
Terminal
History
GitHub
Evidence
```

Example:

```text
┌ PurrCode · purrcode/main · GPT-5 · Build · Auto · Testing ─┐
│                                                             │
│ You                                                         │
│ Add semantic search across uploaded documents.              │
│                                                             │
│ PurrCode                                                    │
│ I am implementing the search API.                           │
│                                                             │
│ ✓ Repository analyzed                                       │
│ ✓ Architecture selected                                     │
│ ● Implementing search API                                   │
│ ○ Frontend search UI                                        │
│ ○ Tests                                                     │
│                                                             │
├─────────────────────────────────────────────────────────────┤
│ Ask PurrCode…                                                │
│ + · Build · Autonomous · GPT-5 · Auto                Send   │
└─────────────────────────────────────────────────────────────┘
```

The TUI must not attempt to render the full supplied IDE layout.

It must preserve the same product semantics in a terminal-appropriate presentation.

---

# 19. TUI ↔ IDE Handoff

## 15.1 TUI to IDE

Commands:

```text
/ide
Ctrl+Shift+I
```

CLI:

```bash
purrcode ide
purrcode ide --session <session-id>
```

Required behavior:

1. Detect a supported VS Code-compatible host.
2. Open the active repository.
3. Activate the PurrCode extension.
4. Open the same session.
5. Preserve model, task mode, permission, and execution state.
6. Do not restart the run.
7. Do not create a new session.

Use a registered URI handler.

## 15.2 IDE to TUI

IDE command:

```text
PurrCode: Open Current Session in Terminal
```

It runs:

```bash
purrcode resume --tui <session-id>
```

## 15.3 Simultaneous clients

Both clients may observe the session.

Only one client may own direct terminal input at one time.

Model, mode, permission, approval, and cancellation updates must synchronize.

---

# 20. Provider and Model Experience

## 16.1 First-run detection

Detect:

- Ollama;
- LM Studio;
- NVIDIA NIM;
- OpenAI;
- OpenAI-compatible providers;
- Azure OpenAI;
- enterprise gateways;
- existing secure credentials.

Do not make a generation request during discovery.

## 16.2 Model picker

Show:

```text
Model
Provider
Local / Remote
Availability
Qualification
Context capacity
```

Example:

```text
● GPT-5                 OpenAI       Remote · Verified
  qwen2.5-coder:7b      Ollama       Local · Verified
  deepseek-v4-pro       NVIDIA NIM   Remote · Available
```

## 16.3 Persistence

Allow:

```text
This request
This session
This repository
Global default
```

## 16.4 Default routing

Normal default:

```text
Use selected model for all roles
```

Advanced role routing remains hidden.

---

# 21. GitHub-Native Completion

## 17.1 Connection flow

Use this priority order:

```text
1. Detect authenticated GitHub CLI
2. Offer one-click reuse
3. Otherwise use OAuth/device authorization
4. Fine-grained PAT reference as explicit fallback
```

The user connects once.

Store credentials in the operating-system keychain or VS Code secret storage.

Never store raw credentials in TOML.

Never send credentials to:

- model context;
- child processes;
- terminal commands;
- logs;
- artifacts.

## 17.2 Required GitHub capabilities

v1.0 must support:

- detect current GitHub remote;
- show repository identity;
- fetch issue context;
- create a safe branch;
- prepare commits;
- push;
- create draft pull request;
- create normal pull request;
- fetch checks;
- display failed checks;
- retry publication;
- preserve local completion when GitHub is unavailable.

## 17.3 Branch policy

Default:

```text
purrcode/<short-task-name>
```

Rules:

- no force push by default;
- no direct protected-default-branch push;
- no silent branch replacement;
- branch creation is visible;
- repository configuration may override naming.

## 17.4 Pull request review

Before creation, show:

```text
Title
Summary
Changed files
Validation
Known limitations
Base branch
Head branch
Draft status
```

PR body:

```text
Summary
Changes
Validation
Risks
Follow-up
```

Do not include hidden chain-of-thought.

## 17.5 Offline behavior

If publication fails:

```text
Local work completed
GitHub publication unavailable
Prepared branch and PR draft preserved
```

The task fails only if GitHub publication is an explicit acceptance requirement.

---

# 22. Environment Provisioning

## 18.1 Existing baseline

Preserve:

- host detection;
- manifest detection;
- missing-tool detection;
- provisioning plans;
- existing-tool verification.

## 18.2 Executable v1.0 managed provisioning

Implement user-local, checksum-verified, atomic provisioning for:

- Node;
- npm / pnpm;
- Python;
- uv;
- JDK;
- Rust toolchain when the repository requires Rust;
- Maven wrapper;
- Gradle wrapper.

Required process:

```text
Detect
→ produce plan
→ authorize
→ download
→ verify checksum/signature
→ install atomically
→ activate environment profile
→ verify version independently
→ continue task
```

## 18.3 Detection-only support

Detection may remain without automatic installation for:

- Go;
- .NET;
- Docker;
- database servers;
- interactive root package installation.

Do not report detection-only support as automatic provisioning.

---

# 23. Automatic Engineering Loop

The Build loop is:

```text
Understand objective
→ inspect repository
→ inspect environment
→ create internal plan
→ modify code
→ run fast validation
→ classify failure
→ repair
→ rerun focused validation
→ run complete required validation
→ prepare review
```

The user must not repeatedly request:

```text
run the tests
fix the failure
continue
review the diff
finish
```

Interrupt only for:

- missing business requirement;
- insufficient authority;
- unavailable required dependency;
- incompatible product decisions;
- budget exhaustion;
- uncertain irreversible external effect.

---

# 24. Testing and Repair

## 20.1 Detection

Support:

- Cargo;
- npm;
- pnpm;
- yarn;
- Bun;
- pytest;
- unittest;
- Maven;
- Gradle;
- Go;
- .NET;
- Make;
- CMake;
- Docker Compose;
- repository CI scripts.

## 20.2 Progressive validation

```text
Static / syntax
→ affected tests
→ module tests
→ full tests
→ integration
→ packaging
→ smoke test
```

## 20.3 Truthful states

```text
Passed
Failed
Timed out
Cancelled
Infrastructure error
Unavailable
Skipped
```

Unavailable and Skipped are never Passed.

## 20.4 Repair loop

```text
Parse failure
→ classify
→ create bounded context
→ repair
→ rerun focused validation
```

Bound:

- attempts;
- model calls;
- wall time;
- changed files;
- commands;
- output;
- test executions.

---

# 25. Brand Asset Requirements

Create:

```text
brand/
├── purrcode-logo-source.png
├── purrcode-logo-horizontal-dark.png
├── purrcode-logo-horizontal-light.png
├── purrcode-mascot-large.png
├── purrcode-cat-head.svg
├── purrcode-cat-head-monochrome.svg
├── purrcode-wordmark-dark.svg
├── purrcode-wordmark-light.svg
└── icons/
    ├── 16.png
    ├── 24.png
    ├── 32.png
    ├── 48.png
    ├── 64.png
    ├── 128.png
    ├── 256.png
    └── 512.png
```

## 21.1 Usage

Full logo:

- README;
- onboarding;
- About;
- extension marketplace;
- documentation;
- release artwork.

Cat-head icon:

- VS Code Activity Bar;
- session avatar;
- status bar;
- app icon;
- compact controls.

Monochrome cat head:

- 16–24px UI;
- dark/light adaptive surfaces;
- terminal documentation;
- high-contrast mode.

TUI:

- use the `PurrCode` wordmark;
- optionally use a simple supported glyph;
- do not render raster terminal art;
- do not depend on emoji appearance.

## 21.2 Asset rules

- Do not redesign the mascot.
- Preserve blue eyes.
- Preserve ragdoll markings.
- Keep the collar/tag where size allows.
- Simplify detail at small sizes.
- Remove excess transparent canvas.
- Remove raster noise around transparent edges.
- Manually review vector derivatives.
- Do not auto-trace and ship without review.
- Ensure icon remains recognizable in monochrome.

---

# 26. Design Tokens

Create one shared token source for IDE, Studio maintenance surfaces, docs, and generated brand assets.

Suggested semantic tokens:

```text
background.primary
background.secondary
background.raised
border.subtle
text.primary
text.secondary
text.muted
accent.primary
accent.hover
status.success
status.warning
status.error
status.info
status.running
```

The IDE extension must prefer VS Code theme tokens where possible.

Do not hardcode a separate theme that becomes unreadable under user themes.

The supplied reference defines the dark-first target, but high-contrast and light themes must remain usable.

---

# 27. Accessibility

Required:

- keyboard-complete TUI;
- keyboard-complete IDE Workbench;
- visible focus;
- screen-reader labels;
- no color-only meaning;
- text status with every status color;
- configurable send/newline behavior;
- copyable output;
- copyable terminal;
- high-contrast support;
- compact-width TUI;
- no icon-only destructive action without accessible label.

---

# 28. Performance Targets

```text
Warm TUI first frame:               < 500 ms
Cold daemon + TUI launch:           < 2 seconds
No startup model request
IDE extension activation:           < 1 second without daemon startup
IDE Workbench first render:         < 300 ms
Cached model picker:                < 300 ms
Keystroke response:                 < 50 ms
Terminal output presentation:       < 100 ms median
TUI → IDE handoff:                  < 2 seconds
IDE → TUI handoff:                  < 2 seconds
Session reconnect:                  < 2 seconds
10,000-event session navigation:    interactive
```

Provider latency must be reported separately from PurrCode overhead.

---

# 29. Security Requirements

Preserve:

- typed actions;
- PawGate authority;
- Claw revalidation;
- action digests;
- isolated effects;
- durable validation;
- idempotency;
- recovery;
- terminal ownership generations.

GitHub:

- no token in model context;
- no token in terminal command;
- no token in child environment;
- no token in config text;
- no token in logs;
- least privilege;
- protected branch safety;
- typed PR publication.

Full Access does not disable:

- identity;
- action binding;
- effect tracking;
- validation;
- recovery;
- operating-system permissions;
- GitHub permissions.

---

# 30. Reliability Requirements

Support:

```text
Durable sessions
Restart recovery
Reconnect
Cancellation
Terminal ownership
Idempotency
Bounded retries
Uncertain-effect handling
Offline local completion
GitHub publication retry
Preserved PR draft
```

IDE reload, terminal closure, or TUI closure must not silently destroy an active session.

---

# 31. Presentation APIs

Extend stable presentation contracts as needed.

Required:

```http
GET /v1/ui/bootstrap
GET /v1/sessions/{id}/summary
GET /v1/sessions/{id}/activity
GET /v1/sessions/{id}/validation
GET /v1/sessions/{id}/changes
GET /v1/sessions/{id}/artifacts
GET /v1/sessions/{id}/github
```

Clients must not independently translate raw event enums into product vocabulary.

Technical evidence remains available through explicit inspection.

---

# 32. Migration From Current VS Code Extension

The current extension is a foundation, not the v1.0 IDE.

Preserve:

- daemon client;
- session actions;
- resume;
- approve/reject;
- model mutation;
- diff endpoints;
- hunk actions.

Replace or demote:

- Sessions tree as the only main view;
- input-box-only task creation;
- input-box-only model selection;
- raw JSON evidence as the default session action;
- manual daemon configuration as the normal path.

Add:

- Workbench conversation;
- composer;
- semantic activity;
- artifact cards;
- mode selector;
- execution-style selector;
- permission selector;
- model picker;
- terminal integration;
- tests;
- native diff workflow;
- GitHub;
- TUI handoff.

---

# 33. Implementation Plan

## PR0 — Baseline Truth

- update implementation status to v0.9;
- create verified capability matrix;
- capture current screenshots;
- run real TUI workflows;
- remove stale documentation;
- confirm current tests and known gaps.

## PR1 — Brand, UX, and Adaptive Contracts

- add supplied reference assets;
- generate reviewed brand derivatives;
- define design tokens;
- define canonical states;
- define terminology;
- define artifact-card contracts;
- add TaskComplexity;
- add WorkflowProfile;
- add WorkflowPlan and WorkflowLane;
- add SearchPolicy;
- add BudgetProfile;
- add UsageRecord;
- extend presentation contracts.

## PR2 — Adaptive Workflow Runtime

- evidence-based task complexity classification;
- Direct / Standard / Ultra selection;
- bounded workflow coordinator;
- lane dependencies;
- isolated write scopes;
- merge/review coordination;
- workflow budgets;
- workflow cancellation and recovery;
- deterministic route evidence.

## PR3 — Provider Pools, Search, MCP, and Usage

- multiple secure credentials;
- credential pools;
- model/provider/key routing;
- health-aware fallback;
- privacy-boundary enforcement;
- Search Off / Auto / Always;
- research trigger evaluation;
- MCP profile management;
- typed MCP actions;
- output limits;
- usage ledger;
- token/cost budgets;
- usage APIs.

## PR4 — TUI Experience Redesign

- preserve runtime behavior;
- implement canonical state language;
- promote plan/result artifacts;
- compact activity;
- simplify session presentation;
- refine composer;
- add workflow/search/budget controls through progressive disclosure;
- add `/ide` placeholder only when IDE support lands behind the same PR sequence.

## PR5 — IDE Workbench Foundation

- daemon bootstrap;
- runtime discovery;
- current session;
- conversation;
- composer;
- semantic activity;
- artifact cards;
- model/mode/permission controls;
- execution style;
- workflow/search/budget controls;
- compact usage summary.

## PR6 — IDE Engineering Integration and Handoff

- editor context;
- native diff;
- diagnostics;
- tests;
- terminal;
- plan/spec documents;
- manual review actions;
- `purrcode ide`;
- URI handler;
- IDE-to-TUI command;
- synchronization;
- terminal ownership.

## PR7 — GitHub-Native Completion

- one-time authentication;
- remote detection;
- issue context;
- branch;
- commit;
- push;
- PR creation;
- checks;
- offline/retry behavior.

## PR8 — Managed Environment Provisioning

- download contracts;
- checksums;
- atomic install;
- environment profiles;
- Node/Python/JDK/Rust;
- post-install evidence.

## PR9 — Product and Efficiency Qualification

- unit tests;
- PTY tests;
- VS Code extension-host tests;
- parity tests;
- adaptive-workflow tests;
- route/fallback tests;
- SearchPolicy tests;
- MCP tests;
- token regression suite;
- GitHub test repositories;
- dogfood;
- performance;
- accessibility;
- release docs.

# 34. Required Testing

## 34.1 TUI

At least:

- 20 real coding tasks;
- 5 Plan → feedback → Build flows;
- 5 terminal/test repair flows;
- narrow and standard terminal sizes;
- restart/resume;
- model switching;
- permission switching.

## 34.2 IDE

At least:

- 15 real tasks started from the IDE;
- editor selection;
- file context;
- diff review;
- test inspection;
- terminal;
- plan revision;
- reload/resume;
- approval;
- cancellation.

## 34.3 Cross-client

At least:

- 10 sessions started in TUI and completed in IDE or the reverse;
- no session divergence;
- no model divergence;
- no task-mode divergence;
- no permission divergence;
- no terminal-ownership conflict.

## 34.4 Adaptive workflow and routing

Required cases:

- simple one-file task selects Direct;
- moderate feature selects Standard;
- migration selects Ultra;
- forced Direct prevents fan-out;
- forced Ultra respects lane limit;
- parallel writers never own overlapping file scope;
- failed lane recovery does not duplicate effects;
- Fixed routing never changes provider/model/key;
- Auto routing selects a qualified route;
- local-to-remote fallback is blocked without permission;
- revoked key is removed from routing;
- rate-limited key falls back only when allowed;
- budget exhaustion stops or asks instead of bypassing the budget.

## 34.5 Search and MCP

Required cases:

- Direct local task performs zero network searches;
- Search Off produces zero search and network-research MCP calls;
- user request for current documentation triggers search under Auto;
- local evidence resolves the task without search;
- unknown external API behavior triggers bounded research;
- official documentation is preferred for technical facts;
- MCP tool calls pass PawGate;
- MCP external writes require appropriate authority;
- oversized MCP output is bounded;
- MCP credential never enters model context;
- disabled MCP server is never selected.

## 34.6 Token and cost efficiency

Required cases:

- every model call produces a usage record;
- per-key/provider/session aggregation is correct;
- cache metrics are recorded only when provided;
- Direct does not use Ultra token budgets;
- unchanged context is not resent without reason;
- terminal and MCP outputs are compacted;
- budget limit is enforced;
- user-facing usage equals raw ledger aggregation;
- context-selection ratio uses measured token counts;
- no unsupported token-savings claim is shown.

## 34.7 GitHub

At least 10 controlled PRs covering:

- public repository;
- private repository;
- issue context;
- protected branch;
- failed checks;
- network failure;
- revoked authorization;
- insufficient permission;
- duplicate publication request;
- publication retry.

---

# 35. Success Metrics

```text
Median time to first useful prompt:        < 5 minutes
Provider/model onboarding success:         > 90%
First coding-task completion:              > 75%
Critical flow without documentation:       > 80%
Unnecessary human questions per run:       < 1
Session recovery success:                  > 99%
TUI ↔ IDE handoff success:                 > 99%
GitHub first connection success:           > 90%
PR creation after local success:           > 95%
Direct tasks with zero unnecessary search:   > 98%
Routing decisions with durable evidence:     100%
Model calls with usage accounting:           100%
MCP calls with bounded output:                100%
Token-budget enforcement accuracy:           100%
```

Telemetry must not collect source code, prompts, credentials, diffs, or terminal contents by default.

---

# 36. Release Gates

## UX

- supplied visual direction is recognizably implemented;
- logo is integrated;
- IDE is conversation-first;
- session list is secondary;
- current model is visible;
- task mode is visible;
- execution style is visible;
- permission is visible;
- plan is a first-class artifact;
- tests and changes are contextual;
- raw event JSON is not the normal view;
- no internal worktree path dominates the UI;
- no placeholder primary feature exists.

## Adaptive orchestration

- simple tasks do not fan out unnecessarily;
- Ultra remains within configured lane and depth limits;
- one writer owns each file scope;
- every workflow route is explainable;
- forced Direct, Fixed routing, and Search Off are enforced;
- provider fallback never crosses a disallowed privacy boundary;
- every model call has usage accounting;
- token and cost budgets are enforced;
- MCP calls are typed, authorized, and bounded;
- no online search occurs under Search Off.

## Runtime

- no duplicate execution;
- no false success;
- no unavailable validation presented as passed;
- no model self-authorization;
- no parallel execution path;
- no session divergence.

## GitHub

- one-time connection works;
- credentials remain secure;
- remote detection works;
- issue context works;
- safe branch works;
- push works;
- PR creation works;
- checks display truthfully;
- offline completion works.

## Quality

- all unit tests pass;
- all PTY tests pass;
- all extension-host tests pass;
- all parity tests pass;
- all GitHub acceptance tests pass;
- required dogfood completes;
- no unresolved P0 UX issue;
- no unresolved P0 security issue.

---

# 37. Definition of Done

PurrCode v1.0 is complete when a new user can:

1. Install PurrCode without Rust.
2. Open a repository.
3. Run `purrcode`.
4. Select or confirm a model.
5. Submit a task.
6. Watch PurrCode inspect, implement, test, repair, and validate.
7. Open the same session in the IDE.
8. See the approved PurrCode visual language and mascot.
9. Review source changes with native IDE diff.
10. Review tests.
11. Open and control a terminal.
12. Revise a plan.
13. Build the revised plan in the same session.
14. Return to TUI without state loss.
15. Connect GitHub once.
16. Attach issue context.
17. Create a safe branch.
18. Commit and push validated work.
19. Create a pull request.
20. Inspect checks.
21. Resume after interruption.
22. Allow PurrCode to select Direct, Standard, or Ultra from task evidence.
23. Force Direct for a simple, no-search task.
24. Run an Ultra task with bounded specialist workflows.
25. Configure multiple secure provider credentials.
26. Use automatic routing without exposing keys.
27. prohibit online search and verify that no search occurred.
28. Allow Auto search to retrieve required current documentation.
29. Use an MCP tool through normal authorization.
30. Inspect actual token, search, MCP, cache, and cost usage.
31. Complete work inside a configured token or cost budget.

The release is not complete if:

- the IDE remains only a Sessions tree;
- the interface still resembles a runtime dashboard;
- the current deliverable is buried in activity logs;
- user-facing status says `paused blocked`;
- model selection is hidden;
- normal setup requires TOML;
- tests require a separate follow-up instruction;
- TUI and IDE diverge;
- GitHub credentials are repeatedly requested;
- GitHub credentials reach the model or shell;
- a PR is created without validation summary;
- the supplied logo is replaced by another icon;
- a simple local task performs unnecessary online search;
- Search Off is not enforced;
- Ultra creates unbounded workflows;
- parallel workflows write the same file scope concurrently;
- a provider or API key changes without an allowed routing policy;
- API keys reach model or tool context;
- MCP output enters context without a bound;
- token usage is unmeasured;
- token savings are claimed without a reproducible baseline;
- user budgets are bypassed through fallback credentials;
- endpoints exist but the real user journey is incomplete.

---

# 38. Codex Execution Rules

Before implementation:

1. Read the current v0.9 implementation.
2. Read the current TUI action registry.
3. Read current presentation APIs.
4. Read the current VS Code extension.
5. Read terminal and validation runtimes.
6. Read provider/model selection.
7. Read current Git boundaries.
8. Run baseline tests.
9. capture baseline screenshots.
10. Record incomplete real-user journeys.

During implementation:

- preserve one authoritative daemon;
- preserve current runtime invariants;
- use native IDE capabilities;
- use supplied assets;
- do not build a second editor;
- do not build a second terminal;
- do not create a second session store;
- do not add Web scope;
- keep Studio maintenance-only;
- keep user vocabulary consistent;
- remove technical noise from normal surfaces;
- use progressive disclosure;
- test every primary interaction;
- keep GitHub credentials isolated;
- keep every provider credential isolated;
- choose the smallest sufficient workflow;
- enforce Search Off exactly;
- search only from evidence under Auto;
- route MCP through PawGate;
- bound MCP, terminal, and search output;
- meter every model call;
- enforce token and cost budgets;
- avoid duplicate context across workflows;
- verify real effects;
- report incomplete work truthfully.

Do not mark a feature complete because:

- a route exists;
- a command exists;
- a button renders;
- a webview loads;
- a PTY process starts;
- a unit test passes;
- a screenshot looks correct.

A feature is complete only when its full user journey works in the real TUI or VS Code extension and is covered by acceptance tests.

---

# 39. Final Product Definition

Surface simplicity:

```text
Repository
Session
Conversation
Composer
Model
Mode
Permission
```

Runtime depth:

```text
Planning
Repository analysis
Environment preparation
Code changes
Terminal
Testing
Repair
Validation
Recovery
Evidence
GitHub
```

Final product rule:

> PurrCode must remain visually calm until the user asks for work, then reveal exactly the code, plan, terminal, tests, changes, decisions, and GitHub actions needed to complete that work.


---

# 40. Design Reference Notes

This PRD uses the following externally established patterns as inspiration, without copying implementation:

- interactive coding agents expose explicit model and permission controls;
- MCP standardizes connections to tools and external data sources;
- LLM gateways commonly provide centralized authentication, model routing, usage tracking, budgets, and rate limits;
- web-search and fetched content can add both request cost and context-token cost;
- bounded tool output and explicit usage accounting are required to keep agent workflows efficient.

PurrCode must implement these capabilities through its own typed runtime, PawGate, Claw, durable evidence, and privacy model.
