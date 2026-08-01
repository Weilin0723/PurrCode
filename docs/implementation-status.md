# Implementation status

Updated: 2026-07-31 (v1.0 feat-adding-IDE in progress)

## v1.0 feat-adding-IDE (in progress)

- Implemented conversation-first VS Code Workbench webview with session title, state, conversation, artifact cards, semantic activity, and composer with mode/permission/workflow controls
- Added daemon HTTP client in TypeScript matching the presentation API contracts (GET /v1/sessions/{id}/summary, /activity, /artifacts, /changes, /validation, /github, /conversation)
- Added Rust domain types for v1.0 adaptive orchestration:
  - TaskComplexity (Simple, Moderate, Complex, Unknown) with evidence-based classification
  - WorkflowProfile (Direct, Standard, Ultra) with bounded parallel specialist lanes
  - SearchPolicy (Off, Auto, Always) with evidence-triggered research
  - BudgetProfile (Economy, Balanced, MaxQuality, Custom) with token/cost enforcement
  - UsageRecord for per-request/model/provider/credential accounting
  - CredentialProfile/CredentialPool with secure reference storage and selection strategies
  - ModelRouteDecision with privacy-boundary enforcement and fallback policies
  - MCPServerProfile with transport/scope/trust and output limits
- Added VS Code extension configuration surface for workflow/search/budget/defaults
- Added production brand assets: icons (16/24/32/48/64/128/256/512px), monochrome cat-head SVGs, wordmark
- Updated README with v1.0 overview and development instructions
- Copied v1.0 PRD and reference assets into repo under docs/prd/ and brand/
- Workbench webview uses native VS Code theming via CSS variables, no hardcoded colors
- Preserves existing TUI as primary interface; Workbench is secondary synchronized view
- No second runtime or session store - all state flows through the daemon
- GitHub-native completion scaffolded (connect, PR creation) - to be implemented in PR7
- TUI canonical state language and workflow/search/budget controls pending (PR4/PR1)
- IDE engineering integration (native diff, diagnostics, tests, terminal, handoff) in progress (PR6)

## v0.8 PR6 — Test orchestration (complete locally)

- `purrcode-test-orchestrator` is the public orchestration crate, with the existing validation
  engine retained behind it for durable v0.7 evidence-schema compatibility. Repository detection
  is bounded and covers Cargo, npm/pnpm/yarn/Bun, pytest/unittest, Maven, Gradle wrapper or system
  Gradle, Go, .NET, Make, CMake, Docker Compose, and declared package/Make CI and smoke scripts.
  Nested projects become module-test stages and directory traversal does not follow symlinks.
- Plans use the explicit syntax/static, focused, module, full-unit, integration, packaging, and
  production-smoke progression. Commands are explicit program-plus-argv actions with network
  disabled; each is durably proposed, judged, exactly authorized, and reverified by Claw before
  execution. Missing tools and undetected stages remain unavailable/not-detected evidence, never
  passing evidence.
- Validation output is classified into compilation, dependency, assertion, configuration,
  environment, compatibility, migration, network, timeout, resource, infrastructure, or unknown
  failures and routed to a bounded specialist repair prompt. The native agent permits three repair
  cycles, reruns only unresolved stages before the complete plan, and pauses with durable routing
  evidence when the budget is exhausted.
- Completion now requires a derived completion event plus every detected required stage passing,
  unless a blueprint explicitly accepts an unavailable required stage. A model's `complete` claim
  alone cannot complete an implementation session.
- A real integration test starts with a failing Make check, observes the failure, routes a model
  repair, requires durable human approval for the write, reruns the affected stage, runs final
  validation, and only then completes. Nine detector/runner tests and 50 agent tests pass, including
  external-symlink rejection and .NET/unittest/custom-CI detection. Platform-specific tools not
  installed on this host are covered by detection tests and are not represented as live passes.

## v0.8 PR5 — Environment provisioning (in progress)

- The environment runtime performs bounded, read-only project detection for Node package managers,
  Maven/Gradle/JDK, Python/uv, Rust, Go, .NET, Docker, and Git. It rejects symlinked or oversized
  manifests rather than following untrusted repository content outside the workspace.
- Host inspection records the real OS, architecture, distribution, shell, package manager,
  elevation/container availability, memory, and disk evidence. Tool discovery prefers repository
  wrappers and managed roots before PATH tools, executes only explicit argv probes in a scrubbed
  environment, applies a five-second timeout, and fails closed when a version cannot be observed.
- `purrcode doctor --repository PATH`, authenticated `POST /v1/environment/inspect`, and Studio's
  explicit Environment surface return one evidence-bearing plan with detected, missing, install,
  and verification records. A local run against this repository observed Git 2.39.3 and Rust
  1.97.1, independently re-probed both, and reported ready from real exit-zero evidence.
- Twelve environment tests cover manifest/version inference, missing requirements, real process
  evidence, and external-symlink rejection; a daemon HTTP test verifies the authenticated surface.
- Checksum-verified managed downloads, atomic installation, repair execution through durable exact
  authorization, and Windows/Linux execution evidence remain pending. Missing tools produce an
  explicit plan and warning; they are never represented as installed or ready.

## v0.8 PR4 — Native PTY terminals (complete locally)

- `purrcode-terminal-runtime` now opens the platform-native backend supplied by `portable-pty`
  (Unix PTY and Windows ConPTY), always spawns an explicit program plus argv, uses absolute working
  directories, and rebuilds child environments from a small system allowlist. Credential-like
  environment keys are rejected and provider credentials are never inherited.
- Long-lived terminals support bounded transcript replay, attach/detach without termination,
  resize, inspect/list, timed wait, process-group termination with escalation, and one-shot command
  execution. Ownership transitions increment a generation and input must match it exactly, closing
  delayed-agent-input races after human takeover.
- The authenticated daemon exposes list/start/get/output/input/resize/attach/detach/owner/stop
  terminal routes. `GET /v1/terminals/{id}/output?since=` serves only the bytes produced after a
  caller's offset and reports when the ring buffer discarded output it never saw, so a live client
  appends instead of re-reading the transcript on a timer.
- Both clients emulate the terminal rather than stripping it: the Workbench renders a real screen
  buffer (`purrcode-tui::terminal`) with tabs, human takeover/return and direct keystroke input,
  and Studio renders the same semantics in `assets/term.js`. The browser retains only its HttpOnly
  Studio cookie; the daemon bearer token remains confined to the shell.
- Eleven runtime tests exercise real PTY commands, interactive input, replay, resize, detach,
  takeover rejection, secret-environment rejection, stop, and exit evidence. A real daemon HTTP
  test drives `/bin/cat`, proves stale generation conflict, observes PTY output, and stops it.
- Real-browser acceptance created an interactive login shell, observed WebSocket output, transferred
  ownership in both directions, confirmed selectable text and no page overflow, stopped the process,
  refreshed Studio, and recovered the complete exited-terminal transcript.
- Terminal records currently survive browser disconnect and Studio restart while the daemon remains
  alive. Durable recovery across daemon/VM restart belongs to the v0.9 Forever Runtime and is not
  represented as complete here. Windows ConPTY compilation/execution remains a cross-platform gate.

## v0.8 PR3 — Workbench (complete)

- Studio now renders a real three-pane Workbench: durable conversation and complete assistant
  output, semantic runtime activity, and a separately selected evidence inspector. Raw event JSON
  is never used as the activity presentation; it is shown only on explicit inspector selection.
- Run cards open durable sessions, and the Workbench loads session metadata, conversation messages,
  and complete events from the authenticated daemon. Follow-up messages use the existing daemon
  route and therefore retain secret detection, session leases, PawGate, and Claw boundaries.
- The browser attaches to the daemon SSE stream through the credential-confining shell proxy.
  Bounded partial assistant output remains visible while generating, durable audit events trigger
  refresh, EventSource reconnect is explicit in the UI, and the daemon bearer token never enters
  JavaScript.
- Diff Review uses the real isolated-worktree patch API and reports a missing worktree truthfully.
- Validation filters only recorded validation/outcome/completion events; absence is displayed as
  pending evidence, never success.
- A failed initial agent configuration is now appended durably as `SessionFailed` before the API
  returns its error. The UI refreshes the durable run list after such a failure instead of leaving
  an apparently active orphan session.
- Real-browser acceptance created a disposable durable run, opened Conversation/Activity/Inspector,
  selected and rendered an exact event payload, exercised Diff Review and Validation unavailable
  states, and proved the three panels collapse to one 700 px column without page overflow. Five
  Studio tests cover HTTP/authentication/SSE proxying and the embedded Workbench contract; the new
  daemon regression test proves initial configuration failure becomes terminal durable state.

## v0.8 PR2 — Studio shell (complete)

- `purrcode-studio-shell` serves a real responsive graphical application from a loopback-only
  Axum server. The dashboard shows daemon health, repository/HEAD/dirty state, durable runs, a
  one-goal run composer, the eleven PRD §10.1 product surfaces, and a visible human-authority
  summary. It uses only white/black canvases with system light/dark mode and a responsive mobile
  navigation layout.
- The browser never receives the daemon bearer credential. `purrcode ui` verifies the daemon and
  Studio API versions before binding, then exchanges a 256-bit one-time bootstrap link for an
  HttpOnly, SameSite=Strict session cookie. The shell consumes the bootstrap exactly once, rejects
  non-loopback binds, rejects cross-origin mutations, applies a restrictive CSP/security headers,
  and forwards only `/v1/*` to the authenticated daemon. PawGate/Claw and the existing daemon remain
  the only execution path.
- Bare `purrcode` launches the terminal Workbench on every platform (v0.9 PRD §3.1, §7.1). Display
  detection no longer rewrites the default interface. `purrcode studio [--remote URL] [--no-open]
  [--repository PATH]` launches the graphical client; `purrcode ui` is a backward-compatible alias,
  and `purrcode tui` is an explicit alias of the bare command. Browser opening uses explicit
  platform argument vectors, never a shell string. `--daemon-token` supports isolated daemon
  instances without changing the secure default token location.
- Daemon health now advertises `studio_api_version` from `purrcode-ui-contracts`; incompatible
  clients fail before the UI server is exposed.
- Three real-HTTP Studio tests prove cookie enforcement, one-time bootstrap consumption, daemon
  credential confinement, authenticated proxying, same-origin mutation enforcement, public-bind
  rejection, and compatibility failure. A real local daemon plus in-app browser acceptance proved
  bootstrap redirect, dashboard rendering, repository inspection, zero sessions/model requests at
  startup, Workbench navigation, and a 700 px layout with no page overflow.
- `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`, and
  `cargo test --workspace --no-fail-fast` pass. The existing live Ollama and disposable macOS
  Keychain tests remain explicitly ignored external checks; they were not represented as passing.

## v0.8 PR1 — UI and runtime contracts (complete)

PRD §24 PR1 pins the shapes of the v0.8 UI↔daemon boundary, the workspace/run
lifecycle, terminal actions, environment provisioning, human authority, and
automatic model selection as six dependency-light contract crates. They carry no
I/O and no tokio dependency, so PR2–PR6 build stable targets against them.

### New crates

- `purrcode-workspace-contracts` — `Workspace` / `WorkspaceId` / `RunId` /
  `RunStatus` / `RepositoryReference` (Local | Remote) / `EnvironmentProfileId`
  / `TerminalId` / `GrantId` per PRD §11.2. Owns the shared newtype ids that the
  sibling crates re-export. `WorkspaceStatus` includes `Disconnected` which is
  *live* so builds and tests survive client disconnect (PRD §11.3). 7 unit tests.
- `purrcode-terminal-runtime` — `TerminalAction` enum (ExecuteCommand /
  StartTerminal / SendInput / ResizeTerminal / InspectProcess / WaitForProcess /
  StopProcess / AttachTerminal / DetachTerminal) with concrete action structs,
  `TerminalOwner` (Human | Agent | Shared), `OwnershipGeneration` for stale-
  input rejection (PRD §12.1), `ManagedProcessSpec`/`ReadinessProbe`/
  `HealthProbe`/`RestartPolicy` (§12.2), and `TerminalSnapshot` for reconnect.
  Pure contracts; the PTY backend lands in PR4. 7 unit tests.
- `purrcode-environment-runtime` — `EnvironmentPlan` (PRD §9.2) with
  required/detected/missing tools, `ProvisionAction`, `EnvironmentCheck`
  verification actions, `ToolKind`, `ToolOrigin` with §9.3 preference ranking,
  `InstallStrategy` and the exact `INSTALL_PREFERENCE_ORDER`,
  `HostEnvironment`/`OsFamily`. `compute_missing`/`satisfies`/`version_at_least`
  fail closed on an empty observed version. 9 unit tests.
- `purrcode-authority-contracts` — `HumanAuthorityMode` (Governed/Elevated/
  Unrestricted, §15.1), `AutonomyGrant` (§15.2), `HumanSubject` with BLAKE3
  identity-claims digest, `CapabilitySet`, `PersistScope` (§15.3), `AgentId`,
  `AzureResourceScope`. The six PRD §15.5 model-restriction invariants are
  pure `assert_model_may_not_*` guards with no allow path: a model can never
  create, widen, re-scope, impersonate, escalate, or hide a grant. 9 unit tests.
- `purrcode-model-selection` — `ModelRole` (§7.2, `as_str()` canonicalizes the
  existing loose role strings), `ModelDeployment`, `QualificationReport`/
  `QualificationStatus` (Unverified is never Qualified), `ModelSelectionPolicy`
  with the §7.4 default, and a pure `select_models` that errors only when the
  policy is unsatisfiable (PRD §7.5), plus `selection_is_stale`. 8 unit tests.
- `purrcode-ui-contracts` — `StudioAction`/`StudioEvent` tagged enums (PRD
  §4/§10), `StudioScreen` registry of all eleven §10.1 screens,
  `STUDIO_API_VERSION = 1` with `is_compatible` exact-major-match gate, and
  `UiContractError`. 7 unit tests.

Total: 47 new unit tests; `cargo fmt --check`, `cargo clippy --workspace
--all-targets -- -D warnings`, and the six-crate test run are green.

## v0.7.0 evaluation runtime, adversarial suite, evidence bundles, and TUI foundation

[... existing v0.7 content truncated for brevity ...]

## v0.6.0 typed actions and deterministic state machine — release candidate

[... existing v0.6 content truncated for brevity ...]

## v0.5.2 provider-routing hotfix — implemented

[... existing v0.5.2 content truncated for brevity ...]

## v0.5.1 connection recovery — implemented

[... existing v0.5.1 content truncated for brevity ...]

## v0.5 usability recovery — Phases 1–6 complete

[... existing v0.5 content truncated for brevity ...]

## v0.4 product redesign — implemented, provider model qualification failed

[... existing v0.4 content truncated for brevity ...]