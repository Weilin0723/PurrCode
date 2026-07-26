# Implementation status

Updated: 2026-07-25 (Epic: Conversational UX, Provider Setup, Skill Discovery — in progress)

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
- Live OpenAI/OpenAI-compatible/Ollama integration tests and installed-model capability discovery
  remain incomplete.

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
- Remote marketplace signature resolution and Windows plugin memory limits remain incomplete.

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
- Provider secrets can be entered through a hidden prompt and stored in macOS Keychain, Windows
  Credential Manager, or Linux Secret Service. Configuration stores references only and secret
  input is zeroized after transfer.
- A daemon-backed VS Code extension builds with strict TypeScript and provides repository/selection
  context, task/plan creation, evidence, approval, lifecycle controls, model selection, and
  current-file isolated diff review.
- Cross-platform CI, signed-release workflow definitions, Homebrew and winget packaging templates
  exist but have not yet been exercised in the upstream release environment.
- Live golden benchmarks now honor the requested whole-task timeout (300 seconds by default)
  instead of silently capping it from a fixture's validation-command budget. Agent prompting also
  directs small fixes toward minimal implementation and validation rather than repeated reads.
- Release validation now gates artifact builds, manual dispatch is tag-guarded, and write/OIDC/
  attestation permissions are scoped to the publish job. Upstream execution remains an external
  release gate.
- Proposed-command editing and exact apply/reject hunk review are implemented. Signed upgrades
  validate archive paths, atomically rotate both binaries, and preserve a tested rollback version.
  Remote skill marketplace discovery, workflow/calendar expressions, and desktop polish remain
  incomplete.
- Durable interval automations are stored in schema migration 2, claimed before daemon launch,
  survive restart, preserve last-session linkage, and expose create/list/enable/disable/run through
  CLI and SDKs. Scheduled sessions retain normal approval boundaries.

## Epic: Conversational UX, Provider Setup, and Skill Discovery — in progress

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
- Conversational interface replaces the session-list view as the primary workspace.
- Message composer with /command detection, history navigation, and Enter-to-send.
- Provider setup wizard (`/connect`) in TUI: select provider type, enter credentials
  via hidden input with zeroize, test connection, select model.
- Skill browser (`/skills`, `/skill-search`): list installed/available skills, inspect
  publisher, permissions, signature, and risk; install with explicit approval.
- Status bar showing model, privacy mode, local/remote indicator, and conversation mode.
- Slash commands: /help, /connect, /providers, /models, /model, /privacy, /plan,
  /build, /review, /diff, /skills, /skill-search, /new, /compact, /cancel, /quit.

### Daemon API expansion
- `GET /v1/providers` — list configured providers
- `POST /v1/providers/test` — test provider connection (redacted output)
- `POST /v1/credentials` — store credential in OS keychain, return opaque reference
- `GET /v1/models` — list available models with local/remote classification
- `GET /v1/skills` — list installed skills from skill-store
- `POST /v1/skills/search` — search registries for matching skills
- `POST /v1/skills/install` — install a candidate skill
- `GET /v1/skills/{id}` — get skill detail
- `DELETE /v1/skills/{id}` — remove skill

### Runtime-core additions
- Conversation types: `Message`, `ConversationState`, `ConversationMode`
- Research/skill lifecycle events: `CapabilityGapDetected`, `SkillInvoked`, etc.
- Qualification types: `QualificationStatus`, `QualificationReport`, `QualificationCaseResult`

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
