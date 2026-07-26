# PurrCode

**A local-first coding agent with an independent, auditable judgment runtime.**

Official repository: [Weilin0723/PurrCode](https://github.com/Weilin0723/PurrCode)

> Models propose. PawGate authorizes. Claw executes. Evidence decides.

PurrCode works in isolated Git worktrees, binds every native action to a durable authorization,
and requires recorded validation before a task can complete. Repository content and model output
are treated as untrusted data, provider credentials are kept out of tool processes, and interrupted
work is recovered conservatively.

PurrCode is the formal product and repository name. Its primary executables are `purrcode` and
`purrcoded`; Rust crates, SDK packages, editor commands, configuration paths, release artifacts,
and package-manager metadata use the same namespace.

## The PurrCode runtime

```text
PurrCode
├── PawGate Judgment Runtime
├── Claw Sandbox
├── Whisker Context Engine
└── NineLives Recovery
```

| Component | Responsibility |
|---|---|
| **PawGate** | Deterministic policy, independent semantic review, exact authorization binding, and human approval gates |
| **Claw** | Shell-free, credential-scrubbed execution inside a worktree-scoped OS sandbox when the host supports one |
| **Whisker** | Bounded repository indexing, context retrieval, sensitive-file filtering, and risk signals |
| **NineLives** | Durable events, checkpoints, restart reconciliation, rollback, and resumable sessions |

中文命名：**PurrCode** 是产品；**PawGate** 是判断与授权层；**Whisker** 负责上下文感知与风险探测；**Claw** 负责受控执行与 sandbox；**NineLives** 负责 checkpoint、recovery 与 rollback。

## Why PurrCode

- **Authorization is enforceable.** A native tool runs only after PawGate durably records the exact
  serialized action and constraints. Claw independently verifies and atomically consumes that
  authorization immediately before execution.
- **Your working tree stays yours.** Agent changes happen in detached session worktrees. PurrCode
  never silently stashes, resets, discards, or overwrites existing work.
- **Completion requires evidence.** Validation can pass, fail, time out, be unavailable, or remain
  undetected; skipped checks are never presented as success.
- **Recovery fails closed.** NineLives reconstructs sessions from an append-only SQLite event log
  and marks interrupted model or tool operations for review instead of replaying uncertain effects.
- **Providers do not become authorities.** Ollama, LM Studio, OpenAI-compatible services,
  enterprise gateways, and the Codex bridge may propose work, but cannot bypass PawGate.

## Install

After the signed `v0.1.0` release is published, macOS and Linux users can download, verify, and
install both binaries with one command:

```bash
curl -fsSL https://raw.githubusercontent.com/Weilin0723/PurrCode/v0.1.0/scripts/install.sh | sh
```

The installer verifies the release archive against `SHA256SUMS` and installs into
`~/.local/bin`. Override the destination with `PURRCODE_INSTALL_DIR` if needed. Windows users can
download the `purrcode-x86_64-pc-windows-msvc.zip` asset directly from the GitHub Release.

Then run `purrcode init` and `purrcode`.

### Install from source

Rust 1.88 or newer is required:

```bash
cargo install --locked --path crates/purrcode-cli
cargo install --locked --path crates/purrcode-daemon
purrcode init
```

`purrcode init` discovers local providers, creates secure local configuration and persistence,
starts the authenticated loopback daemon, and prepares a managed workspace. Run `purrcode`
without a subcommand to open the terminal interface.

### Migrating from LocalJudge / judgeinagent

This repository is the continuation of the earlier LocalJudge/judgeinagent prototype. Before
upgrading, keep a copy of any legacy configuration and session database. The formal PurrCode build
uses the platform PurrCode application directory, `.purrcode/` repository state, `purrcode.toml`,
the `purrcode` Python module, and the `@purrcode/client` TypeScript package. Existing historical
audit records remain valid evidence; they are not rewritten or silently moved.

## Daily workflow

```bash
purrcode plan "Add pagination to the orders API"
purrcode run "Add pagination to the orders API and update tests"
purrcode sessions
purrcode review
purrcode approve
purrcode resume
purrcode rollback
```

Review the isolated diff before explicitly applying or exporting it to your active branch.

## Security model

The trusted path is deliberately small:

```text
Proposed action
  → deterministic PawGate policy
  → judgment and constraints
  → durable exact-action authorization
  → independent verification and single-use consumption
  → Claw execution
  → validation evidence
```

On macOS, PurrCode uses `sandbox-exec`; on Linux it uses Bubblewrap when available. A host without a
supported OS sandbox is reported as restricted process filtering, not as full isolation. See the
[security model](docs/security.md) and [architecture](docs/architecture.md) for exact boundaries.

Provider secrets should be stored with the hidden credential prompt:

```bash
purrcode credential set openai
```

Secrets are stored in macOS Keychain, Windows Credential Manager, or Linux Secret Service.
Configuration keeps only a reference, and tool/plugin processes do not inherit provider
credentials. Environment references remain available for ephemeral CI environments.

## Providers and interfaces

- Local Ollama and LM Studio servers
- OpenAI and OpenAI-compatible APIs
- Enterprise gateways with mTLS, custom CAs, proxies, and external credential commands
- Ratatui terminal interface and headless CLI
- Authenticated loopback daemon with server-sent events
- VS Code extension
- Typed TypeScript and Python SDKs
- MCP/skill host and isolated Codex bridge

Provider setup is documented in [docs/providers.md](docs/providers.md).

## Verification

Run the repository gates:

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
npm test --prefix sdk/typescript
npm test --prefix apps/vscode-extension
PYTHONPATH=sdk/python/src python3 -m unittest discover -s sdk/python/tests -v
purrcode benchmark audit
purrcode benchmark baseline
```

With a qualified provider and running daemon:

```bash
purrcode benchmark live --timeout-seconds 300 --max-tasks 5
```

The report records path accuracy, forbidden mutations, validation status, model calls, approvals,
latency, aggregate accuracy, and safety. External provider qualification, upstream signed-release
execution, and cross-platform runs remain explicit release gates until real evidence exists.

## Project status

PurrCode is currently an **0.1 release candidate**, not a production-approved release. Trusted
runtime contracts and core recovery are implemented; provider qualification, live golden benchmark
quality, and upstream release execution still require environment-specific evidence. The exact,
fail-closed status is maintained in [docs/implementation-status.md](docs/implementation-status.md)
and [docs/production-acceptance.md](docs/production-acceptance.md).

## Documentation

- [Installation](docs/installation.md)
- [Architecture](docs/architecture.md)
- [Security](docs/security.md)
- [Providers](docs/providers.md)
- [Recovery](docs/recovery.md)
- [Troubleshooting](docs/troubleshooting.md)
- [Production acceptance](docs/production-acceptance.md)

## License

Apache-2.0. See [LICENSE](LICENSE).
