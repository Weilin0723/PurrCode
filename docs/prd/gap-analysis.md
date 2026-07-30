# Gap analysis — current codebase vs the remote-runtime final form

Baseline: workspace at v0.8 (issue #28 workbench redesign complete: action
registry + coverage gate, activity/inspector IA, focused approval/review/
recovery surfaces, PTY e2e harness with fake daemon, `ui-actions` CLI).
Target: `docs/prd/remote-runtime-prd.md`.

Legend: ✅ exists · 🟡 partial (seed exists, needs the PRD shape) · ❌ missing.

## 1. Model access

| Capability | State | Where / gap |
|---|---|---|
| Ollama / LM Studio / OpenAI / OpenAI-compatible / enterprise gateway | ✅ | `provider-gateway::ProviderConfig` |
| **Azure OpenAI (endpoint + deployment + api-version)** | ❌ | new `ProviderConfig::AzureOpenai`; request shaping + SSE reuse (PRD §3) |
| **Managed Identity credential (IMDS token, cached)** | ❌ | `AzureCredential::ManagedIdentity`; dev fallback via `az` CLI |
| Keychain/env credential references, secret guard | ✅ | reused as-is for the `api-key` mode |

## 2. Terminal execution

| Capability | State | Where / gap |
|---|---|---|
| Bounded non-interactive command execution (sandboxed) | ✅ | `claw-sandbox` + `CommandAction` (timeout, output caps) |
| Persistent interactive PTY sessions | ✅ | `crates/terminal-runtime::TerminalManager` (33 tests); real `/bin/sh` children, reader/watcher threads |
| Terminal session contract (owner, status, dimensions, transcript policy) | ✅ | `runtime-core::terminal` (`TerminalSessionRecord`, `TerminalStatus`, `TranscriptPolicy`) |
| Ownership generations + stale-input rejection (takeover safety) | ✅ | `OwnershipGeneration`; `TerminalManager::send_input` refuses a stale generation (tested against a real takeover race) |
| Bounded, sequence-numbered transcripts with secret scanning | ✅ | `terminal-runtime::TranscriptRing` (resume-since with explicit truncation) + `redacted_evidence_text` via `provider-import` |
| Process-tree kill / managed-service exemption | 🟡 | `terminal-runtime` signals the whole `setsid` process group (SIGTERM→SIGKILL, tested with a forked grandchild); `ManagedProcess` registry (readiness probes) still new |
| Attach/detach; survive client disconnect | 🟡 | a terminal outlives the caller dropping its handle; **not** yet reachable over a real client connection (that transport is v0.9) |
| Survive daemon restart | ❌ | requires the `terminal-supervisor` process split (v0.9); today a terminal's process ends when the owning `TerminalManager` is dropped — documented as the honest v0.8.5 behavior, not silently faked |
| Typed terminal actions through PawGate approval | 🟡 | `TerminalAction` typed + digestable in `runtime-core` (matches `ProposedAction`'s scheme); PawGate/daemon wiring is still a later PR |

## 3. Autonomous validation

| Capability | State | Where / gap |
|---|---|---|
| Validation runtime with honest status vocabulary | ✅ | `validation-runtime`, `ValidationStatus` (incl. `Unavailable`/`Uncertain`) |
| Build-system detection from repository evidence | ✅ | `crates/test-orchestrator::detect` — pure function over caller-supplied evidence, 13 build systems, every detection cites its evidence paths |
| Progressive test plans (fast → focused → full) | ✅ | `test-orchestrator::plan_for` — ordered phases per build system + `fast_prefix()`/`required_phases()` |
| Test-output parsing (failed test names, diagnostics) | ✅ | `test-orchestrator::parse_test_output` for cargo/pytest/maven/gradle/go/npm family → `ValidationStatus` + `FailureClass` (never trusts exit code alone) |
| Bounded repair loop (attempts/tokens/time/files budgets) | 🟡 | `test-orchestrator::RepairBudget` provides the bookkeeping (attempts/wall-time/changed-files/test-executions, deterministic exceeded-order); the actual parse→classify→repair→rerun *loop* is still a later `agent-runtime` integration |
| Background services with readiness probes | ❌ | `ManagedProcess` (PRD §9) |
| UI never presents non-passes as success | ✅ | v0.8 invariant, tested in TUI + PTY suites |

## 4. Human authority

| Capability | State | Where / gap |
|---|---|---|
| Deterministic policy + contextual judgment + exact-action approval | ✅ | `pawgate-runtime`, digest-bound approval (v0.8 hardened) |
| Governed / Elevated / Unrestricted grant modes | ✅ | `runtime-core::authority::AuthorityMode`; issuance requires a `HumanIdentity` + expiry |
| "PawGate never overrides a valid human grant" | ✅ | `apply_human_authority` — pure function, every bypass returns `overridden: true` in the outcome; advice decisions (`ModifyAction`/`Replan`) are never overridden |
| Model cannot mint/widen grants | 🟡 | `HumanAuthorityGrant::issue` requires a `HumanIdentity` and there is no path that constructs one from model output; the daemon route that actually gates issuance behind Entra/local-TUI auth is still a later PR |
| Grant lifecycle as durable evidence, visible while active | 🟡 | the digest (`HumanAuthorityGrant::digest`) binds the exact scope and changes if the scope widens (tested); wiring grant issuance/use into session events + the header chip is still a later PR |

## 5. Remote operation

| Capability | State | Where / gap |
|---|---|---|
| Local REST daemon + bearer token + SSE events | ✅ | `purrcode-daemon` (loopback) |
| Public HTTPS control plane + Entra ID auth | ❌ | v0.9 PR 1 |
| WebSocket terminal streaming, sequence resume | ❌ | v0.9 (PRD §11) |
| Remote workspace runtime (clone/open/reset/archive) | 🟡 | `repository-engine` worktrees are the seed; `workspace-runtime` crate is new |
| Remote attached TUI (same components) | 🟡 | v0.8 components are reusable by design; remote data source + terminal tabs are new |
| Durable worker queue / schedules / forever runs | 🟡 | CLI `Automation` + `Parallel` subcommands are seeds; queue+leases+restart recovery are new |
| Disconnect/reconnect preserving state | 🟡 | sessions are durable and resumable locally; remote transport layer is new |
| VM deployment profile (systemd units, installer) | ❌ | v0.9 PR 8 |
| Role presets (migration preset) | ❌ | v0.9; full goal compiler is v1.0 |

## 6. Testing infrastructure

| Capability | State | Where / gap |
|---|---|---|
| Ratatui full-screen component tests | ✅ | 316 unit tests in `purrcode-tui` |
| Real-binary PTY e2e harness + fake daemon/provider + artifacts | ✅ | `crates/purrcode-tui-e2e` (7 suites green) |
| Registry-driven acceptance coverage gate + CLI report | ✅ | `ui_actions` + `purrcode ui-actions coverage` (0 incomplete, 0 orphans) |
| PTY suites still to write for v0.8 close-out | 🟡 | `tests/provider.rs`, `tests/model.rs`, `tests/evidence.rs` (referenced by scenarios; `coverage_gate` enumerates them) |
| Remote-profile e2e (client vs fake control plane) | ❌ | v0.9 |
| Real Azure VM acceptance protocol | ❌ | v0.9, following the v0.8 real-terminal template |

## 7. Ordered close-out plan

1. **Finish v0.8**: remaining PTY suites (provider/model/evidence), full-screen
   snapshot suite, UX baseline + acceptance docs, version bump.
2. ~~**v0.8.5 PR A–B**: terminal + grant contracts in `runtime-core`;
   `terminal-runtime` PTY sessions with takeover and bounded transcripts.~~
   **Done.** `runtime-core::terminal` + `runtime-core::authority` (17 tests);
   `crates/terminal-runtime` (33 tests: real spawn, input delivery, stale-
   generation refusal after takeover, process-tree kill, idle/lifetime
   timeouts, secret-redacted evidence export, zero-leak `Drop`).
3. ~~**v0.8.5 PR D**: `test-orchestrator` detection/plans/parsers/budgets.~~
   **Done.** `crates/test-orchestrator` (48 tests: 13 build systems, per-system
   phase plans, cargo/pytest/maven/gradle/go/npm-family output parsing with
   exit-code-contradiction detection, deterministic repair budgets).
   *Not done*: PR C's daemon-side wiring (PawGate/grant consulting
   `TerminalAction`; `AuthorityMode` actually gating a live approval decision)
   — the types and pure logic exist and are tested in isolation, but nothing
   in `purrcode-daemon` calls them yet.
4. **v0.8.5 PR C (remaining) / E / F**: wire `apply_human_authority` and
   `TerminalAction` into the daemon's approval path; `azure-openai` provider;
   workbench terminal tab + autonomous validation loop + PTY e2e.
5. **v0.9**: control plane, supervisor split, WebSocket protocol, workspace
   runtime, remote TUI, forever worker, VM acceptance (PRD §18).
6. **v1.0**: goal compiler, blueprints, generated APIs, ASP control-plane
   profile, canary/rollback.

Workspace-wide status as of this pass: `cargo test --workspace --lib` — every
crate passes (test-orchestrator 48, terminal-runtime 33, runtime-core 49,
purrcode-tui 316, purrcode-tui-e2e 75, plus all other crates); `cargo build -p
purrcode-cli` succeeds; no new warnings introduced.
