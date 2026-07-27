# Implementation status

Updated: 2026-07-26 (v0.5.1 connection recovery implemented; release qualification in progress)

## v0.5.1 connection recovery — implemented

- macOS provider secrets use the actual default login keychain, fixing credential storage on
  accounts where the User-domain lookup returns `errSecNoSuchKeychain`.
- Credential-store failures retain the daemon's bounded redacted diagnostic instead of collapsing
  into a generic rejection.
- First-run Ollama setup uses observed model sizes on low-memory systems, selects the smallest
  installed model, enables single-model coder/judge mode, and persists every discovered model.
- Fresh setup on the affected 8 GB Mac selected `llama3.2:1b`; real Keychain, authenticated daemon
  credential storage, Ollama connection, multi-turn streaming, and unload tests passed.
- Root-cause and real-environment evidence is recorded in
  `docs/reports/v0.5.1-connection-recovery.md`.

## v0.5 usability recovery — Phases 1–6 complete

- Daemon and TUI startup remain generation-free and session-free. A deterministic integration test
  starts the daemon with a live fake Ollama endpoint, performs the same provider-list and repository
  inspection used by the initial TUI, and proves that no provider request, durable session,
  worktree, or checkpoint is created.
- Startup repository presentation is Tier 0: only bounded root metadata is listed. Recursive
  task-relevant indexing remains deferred until a user submits work.
- Authenticated daemon APIs and TUI commands inspect installed/loaded Ollama models through
  `/api/version`, `/api/tags`, and `/api/ps` without generation; `/model unload <model>` and
  `/model unload-all` send native `keep_alive: 0` requests and verify release through `/api/ps`.
- Configurable lifecycle policies support `unload_after_request`, bounded `idle_timeout`,
  opt-in `keep_loaded`, and externally managed lifetime. Native keep-alive updates never load an
  otherwise absent model. Completion, failure, panic, pause, cancellation, and parallel-supervisor
  completion all reach the lifecycle boundary; idle unload will not evict a model reused by a
  different active session.
- The resource governor reads physical/available memory and swap, includes observed loaded-model
  memory, and conservatively permits at most one local inference request across normal agent and
  supervisor paths. It rejects a separate local judge when the detected budget is insufficient.
  Remote requests do not consume the local inference budget.
- Agent leases now own the actual agent future rather than a detached child task, so cancelling a
  lease drops in-flight provider work before durable cancellation and model release.
- Provider import keeps extracted secrets in zeroizing transient storage until the user chooses a
  keychain or environment reference. Native Ollama and explicit OpenAI-compatible modes are
  separate, and bounded redacted diagnostics classify transport, HTTP, schema, framing, model,
  context, memory, and cancellation failures.
- Live output separates rationale content, provider phases/timing, and durable audit events.
  Bounded channels apply backpressure; reconnect restores a bounded snapshot; cancellation
  preserves partial output and releases the local model.
- Hardware-aware recommendations use observed metadata and qualification evidence. Explicit
  Ollama pulls require exact durable approval and support bounded progress and cancellation.
- Installed qualified skills are resolved before external search. Public research, immutable
  download, dynamic qualification, installation, and each MCP invocation retain distinct PawGate
  authorization boundaries.
- Tier 1 begins only after a task. Daemon-owned Tier 2 advances in bounded steps and pauses for
  generation, memory pressure, or degraded scheduler responsiveness.
- Full evidence and the live Ollama acceptance are recorded in
  `docs/reports/v0.5-usability-recovery.md`.

## v0.4 product redesign — implemented, provider model qualification failed

- Phase 1 replaces the single-line, byte-indexed TUI composer with a Unicode grapheme-safe
  multiline editor model.
- Enter inserts a newline; portable Ctrl+G explicitly submits. Ctrl/Cmd/Alt+Enter also submits when
  the terminal reports the modifier; Apple Terminal commonly encodes Ctrl+Enter as plain Enter, so
  the UI does not advertise that indistinguishable sequence as its primary binding. Terminal
  bracketed paste is enabled and a complete paste is inserted as one undoable operation without
  executing commands.
- Multiline history, vertical and word movement, cross-line deletion, indentation/outdent,
  selection replacement, undo/redo, CRLF normalization, and growing scrollable rendering are
  covered by TUI tests.
- Content/secret detection, provider import, provider onboarding, responsive workspace, structured
  runtime cards, and lease-conflict recovery are implemented in the staged Phase 1–7 PR series.
  The running local Ollama service passed real connect and multi-turn streaming tests on 2026-07-26;
  `qwen2.5-coder:7b` failed the complete capability qualification at 2/7 and is not recommended.

### Phase 2 content and secret guard

- A bounded parse-only `purrcode-provider-import` foundation classifies prose, code, logs, and
  provider configuration candidates without executing imported input.
- Common provider keys, named secrets, authorization headers, URL credentials, and nested
  configuration secrets are replaced with a stable redaction token; finding metadata contains
  source spans but never secret values.
- The TUI blocks secret-bearing submissions behind a redaction/import/cancel decision. The daemon
  independently rejects raw secret-like message content before appending a durable event.
- Python and JavaScript imports are syntax-checked with Tree-sitter and only syntax-tree literal
  spans are accepted for static fields. cURL is tokenized without a shell; dotenv, JSON, YAML, and
  TOML use parse-only format-specific paths.
- Imported candidates include provider kind, suggested name, base URL, model, authentication
  reference, API mode, request defaults, custom headers, extra body, local/remote inference,
  confidence, source spans, warnings, and redacted source. Normalization reuses the existing
  provider-gateway configuration types and refuses detected secrets until converted to a reference.
- Fixture tests cover Python, JavaScript, cURL, dotenv, JSON, YAML, TOML, and malformed/dynamic
  source. The editable provider import review remains Phase 4 work.

### Phase 4 provider onboarding

- Provider setup uses a selectable discovery screen and one compact editable form instead of a
  numbered sparse wizard. Local Ollama and LM Studio choices call daemon model discovery and never
  claim a service is running without an observed response.
- `/connect import` and the secret-guard import choice accept bracketed pasted source, parse it
  locally without execution, erase the transient source after parsing, and show editable redacted
  candidate fields and warnings.
- Save performs daemon configuration, a real provider health request with observed latency, and
  model-role assignment. Optimistic placeholder success strings have been removed.
- Saved profiles support `/provider list`, `/provider edit <name>`, `/provider test <name>`, and
  `/provider remove <name>`. Duplicate profile creation requires the explicit edit flow.
- The CLI supports `purrcode provider import <path>` and `purrcode provider import --stdin`; it
  emits a redacted review candidate and never saves implicitly.

### Phase 5 repository-first workspace

- The main TUI has wide (120+), compact (80–119), and narrow (<80) layouts. Wide terminals show a
  bounded workspace path panel beside the timeline; compact/narrow layouts toggle it with Ctrl+B.
- The top bar reports product version, repository/branch, active model, mode, privacy/locality,
  daemon-managed sandbox, session identity, and current runtime phase. Authenticated daemon
  repository inspection reports clean/dirty state without creating a second TUI execution path.
- The workspace panel reads path metadata only, never file contents, is bounded by depth/count,
  skips tool/build directories, and marks sensitive path names. The source tree remains preserved.
- Contextual footer hints change for normal, file, and approval states. Ctrl+D opens the daemon-backed
  diff flow, `?` opens help, and the empty state explains the PawGate → Claw → evidence lifecycle.
- Runtime phase derives from durable daemon events and distinguishes thinking, retrieval, proposal,
  approval, execution, validation, completion, failure, cancellation, and recovery.

### Phase 6 structured runtime timeline

- The conversation surface maps durable session events into semantic plan, action, PawGate,
  approval, Claw, bounded output, validation, checkpoint, recovery, skill, and completion cards.
  Raw event JSON is never used as timeline presentation.
- Ctrl+Up/Down selects timeline cards and Ctrl+Space expands bounded detail. Tool output previews are
  capped, while Ctrl+D continues to open the complete daemon-backed diff flow.
- Pending approvals expose exact-action-bound A approve and R reject shortcuts; both use the daemon
  approval endpoints and preserve PawGate authorization enforcement.

### Phase 7 recovery and polish

- HTTP 409 session lease conflicts stop streaming and open an actionable recovery overlay with
  reconnect, read-only attach, new-session, and technical-detail choices. No competing loop starts.
- Repository-scoped selected-session and unsent-draft state restores after restart. Recovery state
  is written atomically; secret-like values are redacted before disk persistence.
- Ctrl+P and `?` open a keyboard-first command palette with live name/detail/command filtering,
  Up/Down selection, and Enter execution. `NO_COLOR` and dumb terminals receive
  plain-color and ASCII status fallbacks without relying on color alone.
- Composer selection supports Shift+arrows and Page Up/Down. Performance regressions cover a 256 KB
  draft and a 10,000-event timeline with bounded interactive latency.
- The v0.4 release launcher generates its pinned checksums from the exact five native archives in
  the signed-release job before `npm pack`; a stale checksum file cannot silently ship.
- Historical benchmark, installation, provider, recovery, and release evidence is grouped under
  `docs/reports/`; generated qualification artifacts no longer clutter the repository root.

The product and repository are now formally named **PurrCode**. PawGate, Claw, Whisker, and
NineLives name the judgment, execution, context, and recovery subsystems respectively. Primary
commands, crate packages, SDK namespaces, configuration paths, release artifacts, and editor
commands use the PurrCode namespace.

## Milestone 1: trusted runtime contracts — complete

- Cargo workspace and repository instructions.
- Provider-independent action, constraint, decision, authorization, event, and state types.
- BLAKE3 binding of exact serialized action plus constraints.
- Deterministic policy with hard denies and safe command-form checks.
- SQLite WAL event log, versioned initial migration, and integrity check.
- Atomic authorization persistence and single-use consumption.
- Shell-free Tokio process spawning, credential-scrubbed environment, timeout, and bounded output.
- Provider-neutral trait contracts.
- Diagnostic `run`, `policy-check`, `exec`, and `doctor` CLI commands.
- Unit tests for digest mutation, hard deny precedence, executable impersonation, mutating Git
  commands, and at-most-once authorization.

## Milestone 2: provider foundation — in progress

- Responses-compatible HTTP streaming with typed text, tool-call, usage, and lifecycle events.
- Structured output requests use `text.format` JSON Schema and `store = false`.
- OpenAI, OpenAI-compatible loopback servers, and Ollama's OpenAI-compatible endpoint can be
  configured without storing credential values.
- Local-only routing deterministically rejects remote providers.
- Providers marked local must use a loopback URL; non-loopback HTTP is rejected.
- Capability values remain unknown unless configured.
- Model list/use and provider doctor CLI surfaces are implemented.
- Enterprise gateways support mTLS identities, custom CA bundles, proxies, environment-backed
  secret headers, API-key environments, and bounded shell-free OAuth credential commands.
- Transient connection, timeout, HTTP 429, and HTTP 5xx failures are retried before response
  acceptance using a stable idempotency key and bounded backoff.
- `model qualify` measures seven exact-output capabilities, latency, estimated throughput,
  reliable context, and recommended roles.
- Local Ollama and LM Studio model discovery is available through the daemon and conversational
  TUI. Live provider qualification remains an external evidence gate: LM Studio was unavailable
  on the qualification host, and two local Ollama attempts did not produce a completed report.

## Milestone 3: repository isolation — in progress

- Repository inspection records HEAD and dirty source state.
- Session worktrees are detached at HEAD under `.purrcode/worktrees/<session-id>`.
- Dirty source changes are neither copied, stashed, modified, nor discarded.
- Worktree path identity is checked before effect inspection.
- Effect collection includes staged, unstaged, deleted, and untracked paths plus binary patches.
- Native actions compare before/after content hashes and fail on unauthorized paths or file counts.
- Explicit review/diff, binary patch export, conflict-checked active-tree apply, isolated
  branch/commit, leave, discard, and rollback strategies are implemented.
- Initialized local submodules are reproduced in the isolated worktree without network fetching;
  deinitialized submodules are reported as unavailable. Nested submodules use local URL overrides.
- Exact optimistic hunk apply/reject and per-session rollback are covered by repository integration
  tests. Per-action automatic rollback remains incomplete.

## Milestone 4: resumable judged native loop — in progress

- `purrcode run` uses the selected provider and creates an isolated worktree.
- Structured model turns propose exactly one atomic read, write, or delete action.
- Plans, provider calls, proposals, judgments, approvals, execution, and evidence are durable events.
- `resume`, `sessions`, `approve`, and `reject` reconstruct state from SQLite.
- Human-approved writes are exact-digest-bound, single-use, atomic, and optimistic-concurrency safe.
- Validation detection and execution cover Rust, Python, Node, Go, Gradle, and Maven conventions.
- Completion is gated by recorded evidence; unavailable and undetected checks remain explicit.
- Restart recovery marks started-but-unfinished actions uncertain and never retries them.
- Context retrieval and one bounded structured-output repair attempt are integrated.
- Daemon-owned sessions require a separately configured judge role unless the user explicitly
  accepts reduced independence for a single local model.
- The contextual judge receives objective, durable plan revision/current step, pre/postconditions,
  bounded evidence with content digests, prior results, current diff, constraints, and risk.
- Semantic allow decisions must cite supplied evidence IDs. Invented citations fail closed;
  low-confidence and high-risk allows escalate to human review; deterministic denial and approval
  requirements can never be relaxed.
- A second independent outcome judgment gates completion using the final diff and every validation
  category. Blocking evidence forces rework, while low-confidence/high-risk outcomes require
  explicit human review before `SessionCompleted`.
- Validation discovers bounded nested monorepo roots, preserves per-project working directories,
  supports Kotlin/Gradle and Docker Compose configuration checks. Per-action rollback remains
  incomplete.

## Milestone 5: daemon-owned runtime — in progress

- `purrcoded` binds to `127.0.0.1` by default and rejects public binding without an explicit flag.
- A 256-bit bearer token is created with owner-only permissions on Unix.
- Authenticated health, session list/detail, and event APIs are available.
- Daemon startup performs idempotent uncertain-action recovery.
- An integration test proves unauthenticated loopback requests receive HTTP 401.
- Authenticated session event streams are exposed using server-sent events.
- `purrcode serve` runs the same authenticated daemon API.
- Authenticated APIs submit, resume, approve, reject, and cancel native sessions.
- Per-session leases reject concurrent daemon operations; agent jobs construct provider, policy,
  persistence, worktree, and validation components inside the daemon.
- CLI `run`, `resume`, `sessions`, `approve`, `reject`, and `cancel` use the daemon rather than
  creating a second agent runtime.
- Cancellation aborts provider/agent work, persists terminal state, preserves evidence/worktrees,
  and process-group guards kill tool descendants when an execution future is dropped.
- Interrupted model requests are detected at restart and fail closed into explicit recovery review.
- Daemon APIs now expose safe action-boundary pause, resume, manual checkpoints, isolated rollback,
  explicit context compaction, and per-session model selection. IDE clients use these APIs rather
  than creating a second runtime.
- MCP calls and bounded parallel supervisors are daemon-owned. The CLI/TUI starts or reconnects to
  the loopback daemon automatically.

## Milestone 6: repository context — in progress

- Git-ignore-aware bounded indexing with SQLite FTS5.
- Language map, generated-file detection, and sensitive-file classification.
- Tree-sitter symbols for Rust, Python, TypeScript/JavaScript, Java, and Go; Kotlin fallback.
- Import edges, test/source heuristics, manifests, repository instructions, detected build/test
  commands, Git recency, co-change relationships, and current-diff relevance.
- Strict hit and byte budgets; sensitive file contents never enter searchable chunks.
- Retrieved evidence is included in every native model turn and indexing is audit logged.
- Durable automatic and manual context compaction retains objective, current plan, recent actions,
  validation, approvals, and the complete audit log. Incremental indexing, semantic embeddings,
  and compiler/LSP enrichment remain incomplete.

## Milestone 7: Codex Bridge — in progress

- Versioned Codex CLI JSONL adapter with feature and authentication doctor checks.
- Noninteractive execution is restricted to a dedicated PurrCode worktree.
- Active-tree writes and disabling final diff judgment are rejected configuration states.
- Output is bounded and parsed as structured JSON events.
- A fake-worker isolation test proves the worker cannot modify the active tree.
- CLI `codex doctor` and `codex run` surfaces leave every result pending independent diff review.
- Crash-time worker reconciliation, compatibility fixtures, and post-worker validation remain
  incomplete.

## Not implemented

The following validations and product areas are skipped, not passed: live
OpenAI/OpenAI-compatible/Ollama qualification, upstream release workflow execution, opt-in
telemetry, and cross-platform integration execution.

On hosts where no supported OS sandbox is available, the runtime must not broaden automatic
execution beyond carefully audited command forms.

## Reliability and sandbox hardening

- Safe inference requests use bounded retry and idempotency semantics; accepted streams are never
  replayed.
- SQLite supports verified online backups, refuses overwrite, reports schema state, and exposes
  migration preview through the CLI.
- Unix command execution creates a dedicated process group and timeout tests prove background
  children are terminated.
- macOS uses `sandbox-exec` for worktree-scoped writes and denied network access; Linux uses
  bubblewrap when installed. The fallback is explicitly reported as process filtering rather than
  full isolation.
- Every completed command event records the actual sandbox level and backend, and `sandbox doctor`
  reports host capability.

## Skills, plugins, and MCP — in progress

- Repository skill discovery validates the required `SKILL.md` and `manifest.toml` pair, safe
  entrypoints, permissions, platforms, network declaration, secrets, capabilities, and entrypoints.
- MCP discovery and calls are represented as exact internal actions and always require explicit
  human authorization under deterministic policy.
- Authorization is consumed before a separate JSON-RPC child starts; the child receives a
  per-invocation capability token, a scrubbed environment, bounded output, a timeout, and explicit
  working-directory/network grants.
- macOS and Linux MCP children use the available OS sandbox backend; Linux also enforces an address
  space limit.
- `skill list`, `mcp discover --approve`, and `mcp call --approve` provide auditable CLI surfaces.
- Local skill packages install atomically with SemVer validation, file/byte limits, symlink
  rejection, a deterministic content digest, integrity verification, and recoverable uninstall.
- Remote GitHub discovery and archive acquisition are separate exact-authorized actions. Downloads
  are size bounded, deny redirects, reject unsafe ZIP paths/symlinks, verify one manifest and a
  deterministic digest, and remain untrusted until a separately approved install passes static and
  dynamic qualification. Publisher digest signatures and the durable blocklist are enforced.
  Official/organization catalog configuration and Windows plugin memory limits remain incomplete.

## Parallel workers — in progress

- The supervisor validates bounded worker, model-request, and worktree budgets and rejects
  configurations without required isolation.
- It schedules dependency-aware waves concurrently, allocates a distinct detached worktree to
  every worker, preserves failed worktrees, and skips dependents after failure.
- Workers receive only a narrowed workspace view and return results; they receive no merge or
  approval capability.
- Changed-path overlap is detected across isolated outputs. Conflict-free output still requires
  independent review, while overlapping output requires explicit conflict resolution.
- Tests prove two concurrent workers cannot alter the active tree and cannot self-merge.
- Daemon-owned model workers receive one narrowed request and an isolated worktree, pass every
  action through deterministic Judgment, cannot self-approve or merge, emit durable supervisor
  events, and always return independent-review-required.
- Multi-action native workers and supervisor cancellation remain incomplete.

## Daily-use product layer — in progress

- Running `purrcode` without a subcommand opens a daemon-backed Ratatui interface.
- The TUI creates tasks, refreshes durable events, presents plans/actions/deterministic and semantic
  judgments/validation, supports approval, denial, resume, cancellation, and isolated diff review.
- `purrcode init` discovers Ollama or LM Studio models, creates local-only role configuration,
  initializes and integrity-checks persistence, reports sandbox capability, creates a managed Git
  workspace for general local-agent artifacts, launches the daemon, and waits for authenticated
  readiness.
- A single-model installation remains usable only through an explicit reduced-independence setting
  generated with a visible warning.
- A bounded headless `ci` workflow denies interactive approvals and emits an atomic report listing
  every passed, failed, skipped, unavailable, undetected, timed-out, and uncertain validation.
- Typed TypeScript and Python daemon SDKs cover session lifecycle and event streaming and have
  build/unit-test evidence.
- The TUI and daemon support pause/resume, manual checkpoint, isolated rollback, explicit
  compaction, model switch, bounded terminal evidence, and durable plan-only sessions.
- Provider secrets can be entered through a hidden TUI field and stored in macOS Keychain, Windows
  Credential Manager, or Linux Secret Service. Configuration stores references only and secret
  input is zeroized after transfer.
- The daemon provider-test endpoint now probes an already configured provider and reports observed
  health only. It rejects inline secret fields, and model listing no longer invents a default model
  or unknown capabilities. Provider registration, local model discovery, model selection, and role
  assignment are available from the conversational `/connect`, `/model`, and `/role` flows.
- A daemon-backed VS Code extension builds with strict TypeScript and provides repository/selection
  context, task/plan creation, evidence, approval, lifecycle controls, model selection, and
  current-file isolated diff review.
- Cross-platform CI passed on macOS, Linux, and Windows in run 30184357753. Signed-release run
  30184496010 published five `v0.1.0` platform archives, SHA-256 checksums, Sigstore bundles, and
  GitHub provenance; the public macOS ARM64 archive passed the checksum-verifying installer smoke
  test. Homebrew and winget publication remain deferred.
- Conversational/governed-skill CI passed on macOS, Linux, and Windows in run 30189414127.
  Signed-release run 30189538120 published five `v0.2.0` platform archives, SHA-256 checksums,
  Sigstore bundles, and GitHub provenance. The documented public one-command installer verified and
  installed the macOS ARM64 archive, and both binaries reported `0.2.0`.
- Live golden benchmarks now honor the requested whole-task timeout (300 seconds by default)
  instead of silently capping it from a fixture's validation-command budget. Agent prompting also
  directs small fixes toward minimal implementation and validation rather than repeated reads.
- Release validation gates artifact builds, manual dispatch is tag-guarded, and write/OIDC/
  attestation permissions are scoped to the publish job. The complete tag-triggered pipeline has
  now passed upstream for `v0.1.0`.
- Proposed-command editing and exact apply/reject hunk review are implemented. Signed upgrades
  validate archive paths, atomically rotate both binaries, and preserve a tested rollback version.
  Remote skill marketplace discovery, workflow/calendar expressions, and desktop polish remain
  incomplete.
- Durable interval automations are stored in schema migration 2, claimed before daemon launch,
  survive restart, preserve last-session linkage, and expose create/list/enable/disable/run through
  CLI and SDKs. Scheduled sessions retain normal approval boundaries.

## Epic: Conversational UX, Provider Setup, and Skill Discovery — implemented locally

### New crates
- `purrcode-skill-store` — persistent skill library with global/repository/session scopes,
  SQLite metadata, usage tracking, capability-based lookup, and atomic install/remove.
- `purrcode-skill-registry` — registry adapters (official, GitHub, web), candidate ranking
  by source trust, signature validity, publisher, permissions, and license; manifest
  validation and compatibility evaluation.
- `purrcode-web-research` — governed web search with domain policy (allow/deny/approval),
  content sanitization, bounded page retrieval, evidence caching with TTL, and SHA-256
  content digests.

### TUI rewrite
- The production `purrcode` entry point now opens a daemon-backed conversation-first workspace with
  durable multi-turn messages, streamed assistant deltas, a composer/history, slash commands,
  provider and skill views, action/evidence status, real isolated diffs, mouse scrolling, and
  lightweight Markdown/code-block rendering.
- `/connect` performs provider selection, hidden secret entry, local model discovery, provider
  configuration and health testing. `/model` and `/role` select the active model and assign
  planner/coding/judge/reviewer roles. Configuration persists only keychain or environment
  references, never a secret value.
- `/sessions` and `/session <id>` reconnect the conversation UI to durable daemon state; the prior
  session lifecycle, approvals, validation, rollback, and diff controls remain available.

### Daemon API expansion
- `GET /v1/providers` — list configured providers
- `POST /v1/providers/test` — test provider connection (redacted output)
- `POST /v1/credentials` — store credential in OS keychain, return opaque reference
- `GET /v1/models` — list available models with local/remote classification
- `GET /v1/skills` — list installed skills from skill-store
- `POST /v1/skills/search` — propose or execute an exact-authorized registry search
- `POST /v1/skills/download` — propose or execute a separately authorized bounded archive download
- `POST /v1/skills/install/propose` and approval route — inspect and authorize the exact package
- `POST /v1/skills/install` — consume authorization, recheck digest, dynamically qualify, and install
- `GET /v1/skills/{id}` — get skill detail
- `DELETE /v1/skills/{id}` — remove skill
- `POST /v1/research/fetch` — propose or execute a governed, durable-evidence web fetch
- `GET/POST /v1/sessions/{id}/messages` — durable conversation history
- `GET /v1/sessions/{id}/diff` — isolated worktree patch

### Runtime-core additions
- Conversation types: `Message`, `ConversationState`, `ConversationMode`
- Research/skill lifecycle events: `CapabilityGapDetected`, `SkillInvoked`, etc.
- Qualification types: `QualificationStatus`, `QualificationReport`, `QualificationCaseResult`

### Review-closing security and lifecycle work
- Skill storage identity is scope-aware and installs stage, verify, and atomically rename without
  deleting orphaned content. Publisher blocks are durable and case-insensitive.
- Dynamic qualification canonicalizes entrypoints inside the package, requires a strong
  network-isolated Claw backend, binds an exact durable authorization, scrubs inherited secrets,
  snapshots filesystem effects, applies a timeout, and validates the declared output schema.
  Unsupported isolation is reported as `Unverified`, never passed.
- Web policy enforces allow, deny, and approval-required domains on search/fetch; local-only mode
  rejects outbound calls; queries reject credential/path/private-source material; redirects and
  literal and DNS-resolved loopback/private/link-local targets are denied. Validated DNS answers
  are pinned to the request to close rebinding races. Evidence is bounded, content-addressed,
  timestamp-preserving, pseudonymized in redacted exports, and cached durably.
- Capability resolution derives task capabilities instead of using a fixed `core` label, prefers a
  qualified installed skill, and durably records matched/reused/external-search-avoided lifecycle
  events. External search, archive download, installation, and invocation remain separate approval
  boundaries.

### Issue #1 closing qualification (2026-07-26)
- A real local Ollama `llama3.2:1b` run passed daemon discovery, provider configuration, provider
  health, and two provider-backed streamed conversation turns.
- Deterministic coverage now exercises installed-skill-first reuse after reopening the daemon skill
  resolver, durable `InstalledSkillMatched`, `InstalledSkillReused`, and `ExternalSearchAvoided`
  evidence, and the absence of an external skill-search event.
- PawGate invocation coverage proves missing and mismatched authorizations deny, an exact serialized
  authorization allows once, and replay denies. Dynamic qualification additionally rejects
  undeclared contained entrypoints and output-schema mismatches.
- The complete Issue #1 acceptance evidence and exact commands are archived in
  `docs/issue-1-demo.md`. Cross-platform interactive TUI automation remains a later portability
  gate rather than a claim made by this macOS run.

### Distribution and documentation cleanup (2026-07-26)
- The repository no longer tracks macOS metadata files, and generated Rust, npm, and Python build
  outputs remain ignored rather than becoming release inputs.
- A dependency-free `purrcode` npm-compatible launcher selects the published native target,
  restricts downloads to GitHub release hosts, verifies a package-pinned SHA-256 digest, and exposes
  both executables. Release automation includes the launcher tarball in checksums, Sigstore signing,
  provenance, and release upload.
- Homebrew and winget metadata now targets `v0.2.1` with real release digests. The main README is a
  concise English entry point linked to a complete Simplified Chinese translation.

### AGENTS.md
- Added epic-specific rules: no auto-install, no credential-in-context,
  qualification gates execution, PawGate per-invocation, research events durable,
  installed-skill-first discovery, TUI is daemon-backed.

## Enterprise signed policy

- Organization policy packs bind version, issuer, expiration, allowed override fields, policy
  content, BLAKE3 payload hash, and an Ed25519 signature.
- Configuration pins the organization public key and pack path. Missing, expired, malformed,
  tampered, or wrongly signed packs fail daemon/CLI policy loading closed.
- Repository policy merges restrictively by default: allowed commands intersect, deny/approval
  sets union, resource limits take the minimum, and automatic writes require both policies.

## Production release gate

The requirement-by-requirement audit, including all 25 production criteria and 15 mandatory
architecture invariants, is maintained in `docs/production-acceptance.md`. It is intentionally
fail-closed: external provider, signed-release, and cross-platform checks remain gates until they
have real execution evidence.
