# PurrCode

<p align="center">
  <a href="https://github.com/Weilin0723/PurrCode"><img src="purrcode-logo.png" alt="PurrCode" width="420"></a>
</p>

> **A local-first coding agent with an independent, auditable judgment runtime.**
>
> **Models propose. PawGate authorizes. Claw executes. Evidence decides.**

PurrCode is an open-source coding agent for developers who want more control, transparency, and operational safety when using AI to modify code. It works in isolated Git worktrees, records durable evidence, supports local and remote model providers, and treats repository content, model output, downloaded skills, MCP servers, and web content as untrusted.

Unlike a conventional coding chatbot, PurrCode separates model generation, authorization, execution, validation, and recovery into distinct system responsibilities. The CLI, TUI, and the v1.0 native desktop IDE all share one authoritative daemon-owned session model.

## v1.0: the native desktop IDE

PurrCode v1.0 adds its own native desktop IDE — a pure-Rust desktop application, not a browser portal and not an extension inside somebody else's editor. `purrcode ide` (or `purrcode gui`, the same command) opens a real window on the same session the terminal Workbench is running, so you can move between them without losing anything.

- [IDE — the native desktop app](ide.md)
- [Quick Start](quick-start.md)
- [Modes — task and permission](modes.md)

## Install

```bash
curl -fsSL https://raw.githubusercontent.com/Weilin0723/PurrCode/v0.9.0/scripts/install.sh | sh
```

Or via npm: `npm install --global @minaovo/purrcode`. See [Installation](installation.md) for every option, including Windows and building from source.

## Guides

- [Installation](installation.md)
- [Quick Start](quick-start.md)
- [Modes — task and permission](modes.md)
- [Providers and Models](providers-and-models.md)
- [IDE — the native desktop app](ide.md)
- [CLI Reference](cli-reference.md)

## Concepts

- [Architecture](architecture.md)
- [Security](security.md)
- [Recovery](recovery.md)

## Reference

- [Troubleshooting](troubleshooting.md)
- [Development and Testing](development.md)

---

**Repository:** https://github.com/Weilin0723/PurrCode · **Releases:** https://github.com/Weilin0723/PurrCode/releases
