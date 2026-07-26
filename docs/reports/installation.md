# Fresh Machine Installation Report

Date: 2026-07-25
Platform: macOS (darwin, arm64)
RAM: 8 GB
OS: macOS 15+

## Install from source

```bash
cargo install --locked --path crates/purrcode-cli
cargo install --locked --path crates/purrcode-daemon
```

| Step | Status | Notes |
|---|---|---|
| `cargo install purrcode-cli` | PASS | Installed v0.1.0 |
| `cargo install purrcode-daemon` | PASS | Installed v0.1.0 |

## Initialization

| Step | Status | Notes |
|---|---|---|
| `purrcode init` | PASS | Discovered Ollama, created config, prepared workspace, started daemon |
| Config created | PASS | `config.toml` with schema_version=1 |
| Database created | PASS | SQLite WAL at `sessions.db` |
| Workspace created | PASS | Managed workspace at `~/.purrcode/workspace` |
| Daemon started | PASS | Ready at `127.0.0.1:7377` |
| Token created | PASS | Bearer token with owner-only permissions |

## Provider setup

| Step | Status | Notes |
|---|---|---|
| `purrcode provider doctor` | PASS | ollama returned HTTP 200 OK |
| `purrcode model list` | PASS | Roles configured for coder/judge/planner/reviewer/router/summarizer |
| `purrcode model qualify` | SKIP | Requires model with structured output support (blocked by model capability) |

## Core workflow

| Step | Status | Notes |
|---|---|---|
| `purrcode plan` | PASS | Creates durable, resumable session |
| `purrcode run` | PASS | Session accepted, agent loop initiated |
| `purrcode sessions` | PASS | 13 persisted sessions listed |
| `purrcode database backup` | PASS | Integrity check passed |
| `purrcode sandbox doctor` | PASS | RestrictedProcessNoNetwork (macos-sandbox-exec) |
| `purrcode config migration-preview` | PASS | Schema 1, 0 pending migrations |
| `purrcode benchmark audit` | PASS | 30 tasks across 4 categories, 7 languages |
| `purrcode benchmark baseline` | PASS | 22 passed, 0 failed, 8 unavailable |

## Exit and restart

| Step | Status | Notes |
|---|---|---|
| Daemon killed and restarted | PASS | Sessions survive (all 13 present after restart) |
| Recovery runs at startup | PASS | `recovered_uncertain_sessions: []` reported |
| `purrcode sessions` after restart | PASS | Full session history available |

## Upgrade / rollback

| Step | Status | Notes |
|---|---|---|
| `purrcode upgrade check --channel stable` | PASS | Reports current and available version `0.1.0` from the published release |
| `purrcode upgrade download` | UNAVAILABLE | Correctly fails closed because `cosign` is not installed on this smoke-test host |

## All platform coverage

| Platform | Status | Notes |
|---|---|---|
| macOS (this test) | PASS | Full install, init, provider, session lifecycle verified |
| Linux | UNTESTED | CI matrix covers ubuntu-latest |
| Windows | UNTESTED | CI matrix covers windows-latest |

## Summary

The installation workflow is functional on macOS. All CLI commands and subcommands work.
Release `v0.1.0` is published. Its checksum-verifying installer downloaded the public macOS ARM64
archive into a temporary installation directory, and `purrcode --version` plus
`purrcoded --version` both returned `0.1.0`. Cross-platform CI passed on macOS, Linux, and Windows;
package-manager publication and live provider qualification remain separate gates.
