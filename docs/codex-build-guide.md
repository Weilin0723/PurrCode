# Codex build guide

Read `AGENTS.md`, `docs/architecture.md`, the ADRs, and `docs/implementation-status.md` before
editing. Work in dependency order:

1. Domain contracts and persistence.
2. Provider routing and privacy.
3. Repository/worktree isolation.
4. Judgment and tool enforcement.
5. Resumable native loop and validation.
6. Daemon, clients, and Codex Bridge.
7. Production hardening and distribution.

Every milestone requires formatting, Clippy with warnings denied, the full workspace test suite,
specific security regression tests, and an implementation-status update. A missing subsystem or
unavailable validation must be reported as such and never represented as success.

Codex Bridge work must use `.purrcode/worktrees/<session-id>` and must never run against the
active working tree. Its final diff requires an independent judgment before any application
strategy is offered.

