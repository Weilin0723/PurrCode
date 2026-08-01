# PurrCode

[简体中文](README.zh-CN.md) · [Documentation](docs/) · [Latest release](https://github.com/Weilin0723/PurrCode/releases/latest)

[![CI](https://github.com/Weilin0723/PurrCode/actions/workflows/ci.yml/badge.svg)](https://github.com/Weilin0723/PurrCode/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/Weilin0723/PurrCode)](https://github.com/Weilin0723/PurrCode/releases/latest)
[![License](https://img.shields.io/github/license/Weilin0723/PurrCode)](LICENSE)

**A local-first coding agent with an independent, auditable judgment runtime.**

> Models propose. PawGate authorizes. Claw executes. Evidence decides.

PurrCode is a terminal coding agent that works in isolated Git worktrees. Every native action is bound to a durable authorization, checked again immediately before execution, and followed by recorded validation. Repository content, model output, and downloaded skills remain untrusted.

PurrCode v1.0 adds a conversation-first IDE Workbench that shares the same session as the terminal TUI, enabling seamless transitions between terminal and IDE while preserving the proven v0.9 runtime. Key improvements include:

- **Unified session state**: TUI, IDE, and CLI share one authoritative session model
- **IDE Workbench**: Conversation-first interface with artifact cards, semantic activity, and composer controls
- **Adaptive workflow orchestration**: Direct/Standard/Ultra workflow selection based on task evidence
- **Multiple secure credentials**: Provider/model/key routing with budget enforcement
- **Native IDE integration**: Diff, diagnostics, tests, terminal, and GitHub-native completion
- **Usage accounting**: Token/cost/MCP/search tracking with budget enforcement
- **Governed MCP and search**: Policy-bound tool usage with audit trails
- **Production branding**: Official ragdoll-cat logo and visual language

## Install

### npm

```bash
npm install --global @minaovo/purrcode
```

Node.js 18 or newer can also install the signed-release launcher directly from GitHub:

```bash
npm install --global https://github.com/Weilin0723/PurrCode/releases/download/v0.9.0/purrcode-0.9.0.tgz
```

The package selects the correct macOS, Linux, or Windows binary, verifies its pinned SHA-256 digest, and exposes both `purrcode` and `purrcoded`.

### macOS and Linux installer

```bash
curl -fsSL https://raw.githubusercontent.com/Weilin0723/PurrCode/v0.9.0/scripts/install.sh | sh
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

PurrCode defaults to the terminal Workbench. Open a repository, choose a model, describe the outcome, and let it inspect, plan, build, test, and validate — all without leaving your editor.

## Development

This repository contains a **v1.0/feat-adding-IDE** branch with the in-progress IDE Workbench implementation based on the [PurrCode v1.0 Master PRD](docs/prd/PurrCode_v1.0_Codex_Master_PRD.md).

See [`docs/implementation-status.md`](docs/implementation-status.md) for current milestone status.

## Common commands

```bash
purrcode                 # terminal Workbench (default)
purrcode studio          # graphical session view
purrcode plan "Add pagination to the orders API"
purrcode run "Implement pagination and update tests"
purrcode sessions
purrcode review
purrcode approve
purrcode resume
purrcode rollback
```

## Security and verification

PurrCode uses `sandbox-exec` on macOS and Bubblewrap on supported Linux hosts. Weaker host isolation is reported accurately and never presented as a full sandbox. Read the [security model](docs/security.md), [architecture](docs/architecture.md), and [production acceptance audit](docs/production-acceptance.md) before using PurrCode for sensitive repositories.

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

PurrCode is under active development. Consult the implementation status and release notes for verified capabilities and remaining platform-specific gates.

## License

Apache-2.0. See [LICENSE](LICENSE).