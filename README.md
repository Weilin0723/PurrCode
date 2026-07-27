# PurrCode

[简体中文](README.zh-CN.md) · [Documentation](docs/) · [Latest release](https://github.com/Weilin0723/PurrCode/releases/latest)

[![CI](https://github.com/Weilin0723/PurrCode/actions/workflows/ci.yml/badge.svg)](https://github.com/Weilin0723/PurrCode/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/Weilin0723/PurrCode)](https://github.com/Weilin0723/PurrCode/releases/latest)
[![License](https://img.shields.io/github/license/Weilin0723/PurrCode)](LICENSE)

**A local-first coding agent with an independent, auditable judgment runtime.**

> Models propose. PawGate authorizes. Claw executes. Evidence decides.

PurrCode is a terminal coding agent that works in isolated Git worktrees. Every native action is
bound to a durable authorization, checked again immediately before execution, and followed by
recorded validation. Repository content, model output, and downloaded skills remain untrusted.

PurrCode v0.5.2 focuses on dependable daily use: provider profiles reconnect through the OS
keychain, NVIDIA NIM and Ollama use their selected models, sessions recover explicitly, approvals
continue execution, and the terminal stays readable with a high-contrast dark theme.

## Why PurrCode

- **Enforceable authorization:** PawGate approves the exact serialized action and constraints;
  callers cannot bypass the execution adapter's second check.
- **Protected working trees:** agent changes stay in isolated worktrees until you review and apply
  them. Existing uncommitted work is never silently stashed or discarded.
- **Evidence-based completion:** passed, failed, timed-out, unavailable, and skipped validation are
  distinct states. Skipped work is never reported as success.
- **Conservative recovery:** NineLives restores durable sessions and flags uncertain interrupted
  effects for review instead of replaying them.
- **Provider choice without provider authority:** use Ollama, LM Studio, OpenAI-compatible APIs,
  enterprise gateways, or the Codex bridge without letting a model approve its own actions.
- **Governed skills and research:** inspect, qualify, authorize, persist, and reuse skills while
  keeping public web access behind explicit policy.

## Install

### npm

```bash
npm install --global @minaovo/purrcode
```

Node.js 18 or newer can also install the signed-release launcher directly from GitHub:

```bash
npm install --global https://github.com/Weilin0723/PurrCode/releases/download/v0.5.2/purrcode-0.5.2.tgz
```

The package selects the correct macOS, Linux, or Windows binary, verifies its pinned SHA-256 digest,
and exposes both `purrcode` and `purrcoded`.

### macOS and Linux installer

```bash
curl -fsSL https://raw.githubusercontent.com/Weilin0723/PurrCode/v0.5.2/scripts/install.sh | sh
```

This installer verifies the release archive against `SHA256SUMS` and installs into `~/.local/bin`.
Set `PURRCODE_INSTALL_DIR` to choose another destination.

### Build from source

Rust 1.88 or newer is required:

```bash
cargo install --locked --path crates/purrcode-cli
cargo install --locked --path crates/purrcode-daemon
```

## Start in three steps

```bash
# 1. Discover local providers and create secure defaults
purrcode init

# 2. Enter a repository
cd your-project

# 3. Open the conversation-first terminal interface
purrcode
```

Use `/connect` inside the interface to discover Ollama or LM Studio, or configure a remote provider
without editing TOML. Credentials use the operating-system secret store and are not passed to model
context or tool processes.

Paste Python, JavaScript, cURL, JSON, YAML, TOML, or dotenv provider examples with
`/connect import`. PurrCode parses them without execution, keeps extracted secrets transient, and
requires a keychain or environment reference before saving.

## What changed in v0.5

- **Reliable provider routing:** saved keychain credentials, selected remote-provider routing,
  low-memory Ollama defaults, and NVIDIA NIM bounded-generation probes now use the intended profile
  and model.
- **Explicit session recovery:** startup asks whether to resume, open history, or create a new
  session before accepting a task. Terminal sessions are never silently replayed.
- **Approval continuation:** durable approval boundaries wait for the previous daemon lease, reject
  invalid approval requests without corrupting session state, and continue the agent loop after an
  approved action executes.
- **Readable terminal workflow:** the default canvas is opaque black with high-contrast text,
  active provider/model identity is visible, timeline details expand with Space or E, and terminal
  selection remains copyable.
- **Truthful streaming:** content deltas, provider phases, and durable audit events use separate
  bounded channels. Reconnect restores a snapshot; cancellation preserves partial output.
- **Resource-aware local models:** startup never generates or loads a model. Ollama native mode is
  the default, recommendations use observed qualification and memory, and low-memory systems use
  one local request with unload-after-request.
- **Governed capability discovery:** installed qualified skills are checked first. Public search,
  immutable download, dynamic qualification, installation, and every MCP invocation have separate
  exact-action PawGate decisions.
- **Lazy context:** Tier 0 is startup-only metadata, Tier 1 begins after task submission, and
  bounded Tier 2 pauses for generation, memory pressure, or responsiveness.

Useful in-app commands:

```text
/connect import
/model recommend
/model qualify <model>
/model loaded
/model unload <model>
/skills search <query>
/mcp search <query>
/capability add <description>
```

## Runtime model

```text
Model proposal
  → PawGate policy and independent judgment
  → durable exact-action authorization
  → Claw verification and isolated execution
  → validation evidence
  → reviewed application or rollback
```

| Component | Responsibility |
|---|---|
| **PawGate** | Deterministic policy, semantic review, constraints, and human approval gates |
| **Claw** | Credential-scrubbed execution inside a worktree-scoped OS sandbox |
| **Whisker** | Bounded context retrieval, sensitive-file filtering, and risk signals |
| **NineLives** | Durable events, checkpoints, restart reconciliation, and rollback |

## Interfaces

- Conversation-first Ratatui terminal and headless CLI
- Authenticated loopback daemon with server-sent events
- VS Code extension
- TypeScript and Python clients
- MCP and persistent skill host
- Ollama, LM Studio, OpenAI-compatible, and enterprise providers

## Common commands

```bash
purrcode plan "Add pagination to the orders API"
purrcode run "Implement pagination and update tests"
purrcode sessions
purrcode review
purrcode approve
purrcode resume
purrcode rollback
```

## Security and verification

PurrCode uses `sandbox-exec` on macOS and Bubblewrap on supported Linux hosts. Weaker host isolation
is reported accurately and never presented as a full sandbox. Read the [security model](docs/security.md),
[architecture](docs/architecture.md), and [production acceptance audit](docs/production-acceptance.md)
before using PurrCode for sensitive repositories.

Repository checks:

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
npm test --prefix packages/purrcode
npm test --prefix sdk/typescript
npm test --prefix apps/vscode-extension
PYTHONPATH=sdk/python/src python3 -m unittest discover -s sdk/python/tests -v
```

## Documentation

- [Installation](docs/installation.md)
- [Provider setup](docs/providers.md)
- [Architecture](docs/architecture.md)
- [Security](docs/security.md)
- [Recovery](docs/recovery.md)
- [Troubleshooting](docs/troubleshooting.md)
- [Implementation status](docs/implementation-status.md)
- [v0.5 usability recovery evidence](docs/reports/v0.5-usability-recovery.md)
- [v0.5.2 provider-routing hotfix evidence](docs/reports/v0.5.2-provider-routing-hotfix.md)
- [v0.4 redesign acceptance](docs/product-redesign-acceptance.md)
- [Verification reports](docs/reports/)

PurrCode is under active development. Consult the implementation status and release notes for
verified capabilities and remaining platform-specific gates.

## License

Apache-2.0. See [LICENSE](LICENSE).
