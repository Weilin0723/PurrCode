# Production acceptance audit

Updated: 2026-07-24

This is the release gate for the PRD. `Implemented` means the production path and dedicated local
evidence exist. `External gate` means the implementation exists but must still be exercised
against infrastructure or credentials that are not part of the repository. A release is not
approved while any row is not `Implemented`.

## Production acceptance criteria

| # | Requirement | Status | Evidence or remaining gate |
|---:|---|---|---|
| 1 | Representative Python, TypeScript, Java, Go, and Rust changes | External gate | The 30-case catalog in `crates/golden-suite` contains all five languages. Run the live benchmark against each case; repository fixtures still need Java/Kotlin and dirty-tree expansion. |
| 2 | Sessions survive daemon restart | Implemented | Durable event reconstruction, recovery, and daemon integration tests in `crates/purrcode-daemon` and `crates/ninelives-recovery`. |
| 3 | Local-only emits no outbound inference traffic | Implemented | Provider routing rejects remote providers in local-only mode and rejects non-loopback “local” URLs; dedicated provider tests. |
| 4 | OpenAI API-key mode works | External gate | Responses adapter, keychain references, provider doctor, qualification, and live benchmark exist. A real credential qualification run is required. |
| 5 | An OpenAI-compatible local server works | External gate | Loopback adapter and doctor exist. Exercise against a running LM Studio or equivalent server. |
| 6 | Ollama passes qualification | External gate | Ollama discovery/routing exists. Exercise against an installed model. |
| 7 | Codex Bridge uses an isolated worktree | Implemented | `crates/codex-bridge` rejects non-isolated paths and has a fake-worker isolation test. |
| 8 | Codex Bridge never silently modifies the active tree | Implemented | Bridge isolation test plus mandatory independent diff review. |
| 9 | Every native tool action has a judgment record | Implemented | Agent, validation, MCP, and supervisor paths persist judgment before authorization/execution. |
| 10 | Every write has a checkpoint or worktree boundary | Implemented | Detached per-session/per-worker worktrees and durable checkpoints. |
| 11 | Authorized and executed arguments match exactly | Implemented | Canonical action/constraint BLAKE3 digest, persisted single-use authorization, and mutation tests. |
| 12 | Unexpected filesystem effects are detected | Implemented | Before/after hashes, changed-path/file-count constraints, staged/unstaged/untracked/binary effect collection. |
| 13 | Failed/cancelled tasks remain inspectable | Implemented | Cancellation preserves events, evidence, and worktree; failed supervisor worktrees are retained. |
| 14 | Rollback preserves pre-existing work | Implemented | Detached worktree rollback and dirty-source isolation tests. |
| 15 | Tool output is bounded without exhausting memory | Implemented | Bounded asynchronous stdout/stderr capture and truncation evidence in `crates/claw-sandbox`. |
| 16 | Large repositories use bounded context | Implemented | Hit, byte, file, and indexing limits in `crates/whisker-context-engine`; sensitive content exclusion. |
| 17 | Provider interruption is recovered/reported accurately | Implemented | Interrupted requests become explicit recovery review; started executions become uncertain and fail closed. |
| 18 | API keys never appear in logs | Implemented | OS credential-store references, scrubbed child environments, hidden/zeroized CLI input, and redaction tests. |
| 19 | CI never waits for terminal input | Implemented | Bounded headless mode rejects approval-required work and emits an atomic evidence report. |
| 20 | Installer, updater, and migrations are tested | Implemented | Signed `v0.1.0` artifacts were published, and the checksum-verifying installer downloaded the public macOS ARM64 archive and ran both installed binaries successfully. Atomic upgrade/rollback and migration tests also pass. |
| 21 | Cross-platform integration tests pass | Implemented | CI run 30184357753 passed Rust checks/tests on macOS, Linux, and Windows plus both TypeScript clients and Python; signed-release run 30184496010 built all five platform artifacts. |
| 22 | Security invariants have dedicated tests | Implemented | See invariant matrix below and crate-level regression tests. |
| 23 | No placeholder production successes | Implemented | Validation distinguishes passed, failed, unavailable, undetected, skipped, timed out, and uncertain; catalog-only expectations report unavailable rather than passed. |
| 24 | Installation/providers/security/recovery/troubleshooting docs | Implemented | `docs/installation.md`, `docs/providers.md`, `SECURITY.md`, `docs/recovery.md`, and `docs/troubleshooting.md`. |
| 25 | Final report lists skipped/unavailable validation | Implemented | `ValidationReport`, CI JSON report, daemon events, and TUI distinguish all evidence states. |

## Mandatory architecture invariants

| # | Invariant | Status | Enforcement |
|---:|---|---|---|
| 1 | Native tools require persisted authorization | Implemented | `ToolRuntime::execute` consumes an authorization from SQLite before spawning. |
| 2 | Authorization binds exact arguments and constraints | Implemented | Canonical BLAKE3 action/constraint digest. |
| 3 | Model output cannot modify hard policy | Implemented | Deterministic policy runs independently and semantic judgment may only restrict. |
| 4 | Local-only cannot use remote inference | Implemented | Provider router fail-closed locality checks. |
| 5 | Codex Bridge cannot write the active tree | Implemented | Canonical isolated-worktree validation. |
| 6 | Workers cannot approve or merge their output | Implemented | Narrow worker capability plus independent review disposition. |
| 7 | Completion requires recorded validation | Implemented | Outcome judgment and completion gate consume durable evidence. |
| 8 | Skipped is never passed | Implemented | Separate validation status variants end to end. |
| 9 | State survives daemon restart | Implemented | SQLite WAL event reconstruction and restart recovery. |
| 10 | Uncertain execution fails closed | Implemented | Recovery records uncertainty and never retries automatically. |
| 11 | Repository content is untrusted | Implemented | Structured prompts, bounded context, safe paths, shell-free execution, and effect checks. |
| 12 | User uncommitted changes are never silently discarded | Implemented | Source inspection plus detached worktrees; no stash/reset of source. |
| 13 | Credentials are hidden from arbitrary tools | Implemented | Scrubbed environment and explicit per-provider secret resolution only in provider process. |
| 14 | External tools/plugins/MCP pass through Judgment | Implemented | Exact internal actions and single-use authorization at the MCP/plugin boundary. |
| 15 | Local failure cannot silently trigger remote fallback | Implemented | Explicit provider selection and policy-controlled escalation; local-only hard rejection. |

## Release decision

The offline runtime is published as release candidate `v0.1.0`, but the full production acceptance
gate remains open. Supply credentials locally through `purrcode credential set` to execute criteria
1, 4, 5, and 6. Secrets must never be pasted into chat or configuration.
