# PRD — Remote Developer Workstation, Native Terminal Runtime, and Autonomous Testing

Status: reviewed and corrected. This supersedes the draft addendum.
Audience: implementation agents (Codex/Claude) and maintainers.
Baseline: PurrCode v0.8 (verified local workbench, issue #28) is the foundation;
nothing here weakens a v0.8 invariant.

## 0. Review verdict and corrections applied

The draft addendum was directionally sound. The following defects were corrected
in this version — implementers must follow this document where the two differ:

1. **Azure OpenAI provider was missing entirely.** The core ask is that PurrCode
   natively supports Azure-hosted models. §3 adds the `azure-openai` provider
   with Managed Identity. It is the first deliverable, not an afterthought.
2. **App Service Plan cannot host the terminal runtime.** ASP sandboxes do not
   allow PTY-heavy process trees, cgroups, or systemd scopes. §2.4 restates the
   platform matrix honestly: interactive/terminal features require a VM (or
   Container Apps); ASP hosts control-plane/headless mode only, in v1.0.
3. **A second validation vocabulary was introduced.** The draft's `TestStatus`
   duplicated `purrcode_runtime_core::ValidationStatus`. §8.4 reuses the
   existing enum; infrastructure failures map to `Unavailable`/`Uncertain` with
   a `FailureClass` refinement. One vocabulary, no drift.
4. **Terminal survival across daemon restart was overstated.** A PTY owned by
   the daemon process dies with it. §4.3 mandates the supervisor split: terminal
   processes run under `terminal-supervisor` (systemd scope on Linux) so the API
   daemon can restart beneath them; without a supervisor, restart reconciliation
   marks terminals `RecoveryRequired` — it never pretends they survived.
5. **Reconnection replay was underspecified.** §11.2 requires sequence-numbered
   output with bounded ring-buffer resume: the client presents its last sequence
   and receives only what it missed, or a truncation notice. No duplicates, no
   silent gaps.
6. **Build-system detection provenance.** Detection must consume repository
   evidence through the existing repository engine — never a model guess and
   never a second filesystem scanner (§8.1).
7. **The intake ask was implicit.** §6 makes it explicit: one structured
   provisioning intake collects managed-identity/endpoint/model/budget answers
   up front; after intake the runtime does not come back with ordinary
   implementation questions.
8. **Human authority wording sharpened** (§13): PawGate advises and records; it
   never overrides a decision inside a valid human grant. Unrestricted mode
   removes every policy veto. What it can never remove: typed actions, identity
   attribution, recording, and secret redaction in artifacts.
9. **Roadmap resequenced.** v0.8 stays as shipped (issue #28 scope). A new
   v0.8.5 delivers the local-first architecture (terminal runtime, test
   orchestrator, Azure provider, authority grants) so v0.9 remote work composes
   proven parts instead of inventing them remotely. Architecture first.
10. **Windows scope bounded.** ConPTY support ships through `portable-pty`, but
    v0.9 acceptance targets Linux VMs; Windows VM acceptance is v1.0.

## 1. Product requirement

An Azure deployment of PurrCode must support both:

```text
Permanent autonomous agent service   (headless forever mode)
Interactive remote coding workbench  (remote attached mode)
```

A deployed PurrCode VM provides an experience comparable to Codex CLI or Claude
Code: connect from the local TUI/CLI, open remote repositories, watch the agent
work in real time, let it run terminals/builds/tests autonomously, attach to any
terminal, take control, hand control back, disconnect while work continues, and
reconnect without losing state — under the same approval, authority, recovery,
validation and evidence model as local mode. The remote product is not a
reduced product.

## 2. Operating modes

### 2.1 Local attached (exists today)

```text
Local TUI → local purrcoded → local repository and terminals
```

### 2.2 Remote attached

```text
Local TUI/CLI → authenticated remote protocol → purrcoded on Azure VM
             → remote workspaces, terminals, agents
```

The local client is a presentation layer. All repository access, terminal
execution, model calls, validation and durable state live on the VM.

### 2.3 Headless forever

```text
REST / queue / schedule → control plane → autonomous runtime
                       → workspace + terminals + agents + validation
```

A run started headlessly can later be opened in remote attached mode for
inspection or intervention. The two modes are views over the same durable run.

### 2.4 Platform matrix (corrected)

| Capability | Azure VM | Container Apps | App Service Plan |
|---|---|---|---|
| Control plane REST/SSE | yes | yes | yes (v1.0) |
| Headless forever runs | yes | yes | limited (v1.0) |
| Interactive PTY terminals | yes | possible | **no** |
| Process isolation (cgroups/systemd scopes) | yes | container-level | **no** |
| Background services (db, dev server) | yes | yes | **no** |

v0.9 targets the VM profile. ASP is a v1.0 control-plane profile only, and the
docs must say plainly which features it lacks.

## 3. Azure-native model access (new)

Add to `provider-gateway`:

```rust
ProviderConfig::AzureOpenai {
    /// https://{resource}.openai.azure.com or an AI Foundry endpoint.
    endpoint: Url,
    deployment: String,
    api_version: String,          // e.g. "2024-10-21"
    credential: AzureCredential,  // see below
    capabilities: BTreeMap<String, ModelCapabilities>,
}

pub enum AzureCredential {
    /// `api-key` header from the OS keychain (existing credential store).
    KeychainKey { name: String },
    /// Entra ID bearer token from the VM's Managed Identity (IMDS), scope
    /// https://cognitiveservices.azure.com/.default, cached until expiry.
    ManagedIdentity { client_id: Option<String> },
    /// Developer convenience: token via `az account get-access-token`.
    AzureCli,
}
```

Requirements:

* request shaping: `{endpoint}/openai/deployments/{deployment}/chat/completions?api-version={v}`;
  streaming uses the same SSE format as OpenAI and reuses the existing decoder;
* HTTPS enforced by the existing `ProviderConfig::validate` rule;
* raw keys never in config files — keychain reference or Managed Identity only
  (the existing secret-guard and import rules apply unchanged);
* provider tests, TUI provider-setup entry, and a real connection test, exactly
  as for every other provider;
* token acquisition failures are provider failures with a retry path, never
  silent fallbacks to another credential.

## 4. Native terminal runtime

New crate: `crates/terminal-runtime`.

Responsibilities: persistent PTY sessions; non-interactive command execution;
interactive processes; input and resize; attach/detach; output streaming;
lifecycle and resource limits; cancellation; durable terminal metadata; bounded
transcript persistence; recovery after client disconnection; reconciliation
after daemon restart.

### 4.1 Platforms

Linux/macOS: PTY. Windows: ConPTY (via `portable-pty`). Never emulate a
terminal by concatenating stdout: interactive tools (`cargo test`, `pytest`,
watch modes, REPLs, dev servers, migration CLIs, debuggers) get a real TTY.

### 4.2 Session contract

The contract types live in `runtime-core` so daemon, TUI and API share one
definition (see draft §4 for the full field list — adopted as written, plus):

* `OwnershipGeneration(u64)`: every ownership transition increments it.
  `SendInput` binds `(terminal_id, expected_generation)`; the runtime rejects
  input carrying a stale generation. This is what makes human takeover safe:
  after takeover the agent's generation is stale and its queued input is
  refused, recorded, and visible.
* Every ownership transition is durable evidence.
* `TranscriptPolicy` includes mandatory secret scanning before persistence —
  transcripts are artifacts, and artifacts never contain raw secrets.

### 4.3 Survival semantics (corrected)

* Client disconnect never terminates a terminal.
* Daemon restart: terminals run under `terminal-supervisor` (a separate
  process; systemd scope per terminal on Linux) so the daemon can restart
  beneath them and re-adopt by pid + generation. Where the supervisor is not
  deployed (dev laptops), restart marks running terminals `RecoveryRequired` —
  the UI shows recovery, never a fake "still running".
* Stopping a command kills its entire process tree (`killpg`/Job Objects)
  unless the process was registered as a managed background service.

### 4.4 Human takeover

Take control / send input / return control / terminate / open another terminal.
During human control the agent's input is refused (stale generation), the
process continues, output stays visible, and the transition is durable. On
return, the agent receives current state plus the bounded transcript tail.

## 5. Typed terminal actions

Adopt the draft §5 action set (`ExecuteCommand`, `StartTerminal`, `SendInput`,
`ResizeTerminal`, `WaitForProcess`, `InspectProcess`, `StopProcess`,
`AttachTerminal`, `DetachTerminal`) with these bindings:

* all actions are `ProposedAction`-style typed values that digest with their
  constraints — the existing exact-action approval machinery applies unchanged;
* `ExecuteCommand` is for bounded batch work (build, test, lint); `StartTerminal`
  for long-lived interactive processes;
* native typed actions (file read/write, search, diff, git metadata) remain the
  fast path; the runtime never shells out where a native action exists, and
  never rebuilds build tools natively.

## 6. Provisioning intake (new)

At agent/run creation the control plane collects, once, in one structured step:

```text
model access        Azure endpoint + deployment + credential mode
identity            managed identity client id (or keychain credential name)
repository access   URL + credential reference
authority           Governed | Elevated(capabilities) | Unrestricted
budgets             tokens/run, wall-clock, changed files, test executions
schedule/trigger    REST | queue | cron
notification        where results and questions go
```

After intake the runtime does not ask ordinary implementation questions. It may
come back only when: required information is genuinely unavailable; authority is
insufficient for a proposed action; an external dependency is unreachable; a
business decision cannot be inferred safely; a budget is exhausted. Each
question is a durable decision boundary (like approval), not a chat message.

## 7. Automatic development workflow

Default autonomous loop (draft §15 adopted):

```text
understand → inspect → plan → edit → fast validation → classify failure
→ repair → focused tests → full required validation → review effects → result
```

"Run the tests", "fix the error", "retry", "show the diff" are runtime
responsibilities, never user prompts.

## 8. Automatic test orchestrator

New crate: `crates/test-orchestrator`.

### 8.1 Detection (corrected)

Build-system detection consumes repository evidence (manifest files, lockfiles,
CI definitions) surfaced through the existing repository engine. Detection
output carries the evidence paths that justified it. No model guessing, no
second scanner.

Detect at minimum: Cargo, npm/pnpm/yarn/bun, Python (pytest/unittest via
pyproject/requirements), Maven, Gradle, Go, .NET, Make, CMake, Docker Compose,
repository-defined scripts.

### 8.2 Test plans and progressive validation

`TestPlan` with phased execution (dependency check → static analysis →
compilation → focused tests → module tests → full unit → integration →
packaging → smoke → security), budgets, and a failure policy. Small edits run
the fast prefix; completion requires everything the blueprint demands. Repair
reruns only failed/affected phases.

### 8.3 Result parsing

Exit codes are never the only evidence. Parse failed test names, compiler
diagnostics, durations, timeouts, cancellations, resource exhaustion, missing
tools, environment failures — normalized per §8.4, with raw output retained as
evidence.

### 8.4 Status vocabulary (corrected)

Reuse `purrcode_runtime_core::ValidationStatus` (`Passed`, `Failed`,
`TimedOut`, `Unavailable`, `Uncertain`, …). Add a `FailureClass` refinement
(compilation, dependency, assertion, configuration, environment, network,
migration, compatibility, timeout, resource-exhaustion, infrastructure,
unknown) used for repair routing. Infrastructure failure maps to
`Unavailable`; ambiguous outcomes map to `Uncertain` and are never presented
as passes (v0.8 invariant).

### 8.5 Bounded repair loop

parse → classify → select specialist → bounded context → propose → execute →
rerun focused validation. Hard budgets on attempts, tokens, wall time, changed
files and test executions. Exhaustion is a durable decision boundary for the
human, not an infinite loop and not a silent give-up.

## 9. Background services

Adopt draft §8 (`ManagedProcess` with readiness probes — TCP/HTTP/log
pattern/health/custom). Start, await readiness, test against, collect logs,
stop reliably, recover or clean up after failure. Managed services are exempt
from process-tree kill only because they are explicitly registered and owned.

## 10. Remote workspace runtime

Adopt draft §9 (`workspace-runtime`: create/clone/open/inspect/reset/archive/
delete/attach; list terminals and processes). Workspaces survive client
disconnection always; surviving VM restart depends on the storage profile, but
durable metadata and recovery state survive unconditionally.

## 11. Remote client protocol

### 11.1 Transport and auth

REST for commands/state; WebSocket for bidirectional terminal I/O (output,
input, resize, attach/detach, takeover); SSE remains for durable agent events.
Authentication: Entra ID device login, Azure CLI credential for developers,
service-to-service tokens for automation. Users never copy bearer tokens by
hand. Control plane is HTTPS-only.

### 11.2 Reconnection (corrected)

Terminal output frames carry monotonic sequence numbers backed by a bounded
server-side ring buffer. On reconnect the client presents its last sequence and
receives exactly the missed frames, or an explicit truncation notice plus the
bounded tail. Ownership is preserved; commands are never restarted
automatically; duplicates are impossible by construction.

## 12. Remote workbench UX

The remote attached experience is the v0.8 workbench plus: a VM identity in the
header, terminal tabs (agent/build/tests/server/human) with attach/take
control/return/terminate/copy/inspect-grant, and semantic progress derived from
orchestrator state ("Compiling module 4 of 7", "3 tests failed", "Repairing
null-handling regression") with raw output one level down in Terminal/Evidence
views. Local and remote screens share components; a capability the remote mode
lacks must be visibly absent with a reason, not silently missing. All v0.8
release gates (60-column usability, NO_COLOR, full-screen snapshots, PTY
journeys) apply to the remote screens too.

## 13. Human authority (sharpened)

Three modes, one principle: **PawGate advises and records; it never overrides a
decision inside a valid human grant.**

* **Governed** — today's behavior: policy plus exact-action approval.
* **Elevated** — the grant enumerates capabilities and allowed programs
  (draft §12 YAML adopted); listed actions skip per-action approval, everything
  else falls back to Governed.
* **Unrestricted** — no policy veto and no repeated approvals for anything the
  VM process identity can do. The human asked for everything; PurrCode complies.

What no mode can remove:

* actions stay typed and digested;
* the grant id and human identity are attached to every action;
* everything is recorded as durable evidence;
* secrets stay redacted in transcripts, logs, and bundles;
* **the model can never create, widen, extend or reactivate a grant** — grants
  are created only through the authenticated human channel (TUI confirmation or
  Entra-authenticated API call), have expiry, and are revocable at any time;
* actual capability is still bounded by the OS process identity and Azure RBAC
  — a grant grants PurrCode's authority, not new machine authority.

Grant lifecycle events (created, used-for-first-bypass, expired, revoked) are
durable evidence and visible in the workbench header while active.

## 14. REST and WebSocket API

Adopt draft §13 surface (workspaces, terminals + WS stream, test plans/runs)
with additions: `POST /v1/grants` + `DELETE /v1/grants/{id}` (human channel
only), `GET /v1/runs/{id}/evidence`, and API versioning with compatibility
negotiation on connect.

## 15. Reference example — migration agent

Draft §14 adopted as the canonical acceptance narrative, with one correction:
in v0.9 the specialist set is a **role preset** ("migration" preset wiring
analyzer/planner/build/test/repair/PR roles); the natural-language goal
compiler that synthesizes novel agent sets is v1.0. The user experience in the
example is unchanged.

## 16. Resource and process isolation

Adopt draft §16 (`ProcessLimits`; Linux: systemd scopes, cgroups v2, process
groups, workspaces, optional containers; Windows: Job Objects, ConPTY, process
trees). Cancellation kills the whole owned tree except registered managed
services.

## 17. Testing requirements

Draft §17 adopted (terminal runtime, orchestrator, remote UX test lists), plus:

* every new user-facing capability registers in the v0.8 action registry with
  acceptance scenarios — the coverage gate extends to remote actions;
* the PTY e2e harness grows a remote profile (client TUI against a fake control
  plane) before any real-Azure acceptance;
* ownership-race, stale-input, sequence-resume and grant-bypass tests are
  release-gating;
* real Azure VM acceptance follows the v0.8 real-terminal protocol
  (documented journeys, screenshots, durable results).

## 18. Delivery roadmap (resequenced)

**v0.8 — Verified local workbench** *(shipped; issue #28 scope unchanged)*

**v0.8.5 — Local terminal & autonomous validation (architecture first)**
* `runtime-core`: terminal contracts, ownership generations, authority grants;
* `crates/terminal-runtime` (local PTY, takeover, transcripts, limits);
* `crates/test-orchestrator` (detection, plans, parsers, repair budgets);
* typed terminal actions through PawGate + grants;
* `azure-openai` provider with Managed Identity;
* local workbench terminal tab + autonomous validation loop;
* everything unit/PTY-tested locally — no cloud dependency to develop or test.

**v0.9 — Remote VM workbench & forever runtime**
* Entra auth + public control plane (HTTPS, tokens never hand-copied);
* `workspace-runtime`; terminal supervisor split; WebSocket terminal protocol;
* remote attached TUI (same components, terminal tabs, takeover);
* durable worker queue, schedules, headless REST runs;
* disconnect/reconnect with sequence resume; VM deployment profile (systemd);
* role presets (migration preset); real-VM acceptance suite.

**v1.0 — Production agent factory**
* natural-language goal compiler + agent blueprints; production qualification;
* generated REST APIs per agent; deployment automation; ASP control-plane
  profile; versioning/canary/rollback; Windows VM acceptance.

### v0.8.5 PR sequence

```text
PR A  runtime-core terminal + grant contracts (types, digests, tests)
PR B  terminal-runtime crate (PTY sessions, takeover, transcripts, kill-tree)
PR C  typed terminal actions + PawGate/grant integration
PR D  test-orchestrator (detection, plans, parsers, budgets)
PR E  azure-openai provider (config, auth, streaming, setup UI, tests)
PR F  workbench terminal tab + autonomous validation loop + PTY e2e
```

### v0.9 PR sequence

Draft §19 adopted (remote client contract → terminal supervisor/protocol →
workspace runtime → remote UX → forever worker → Azure acceptance), minus the
parts delivered in v0.8.5.

## 19. Definition of done (final form)

Draft §20 adopted in full, plus:

* an unrestricted grant is honored end-to-end: PawGate never vetoes inside it,
  and every bypassed decision is attributed and recorded;
* the model cannot mint or widen a grant (negative tests exist);
* no transcript, log, or evidence bundle contains a raw secret;
* remote and local interfaces pass the same snapshot/PTY gates;
* a migration-preset run completes headlessly and is then opened, attached,
  taken over, returned, and completed from the TUI — one continuous run.

The VM product is not done while: it only exposes REST; terminals die on client
disconnect; users must request each test/retry manually; the agent cannot parse
and repair build failures; remote behaves differently from local; takeover
races agent input; success is inferred from model claims; or runs cannot be
reopened and inspected.
