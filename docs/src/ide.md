<p align="center">
  <a href="README.md"><img src="purrcode-logo.png" alt="PurrCode" width="180"></a>
</p>

# IDE — the native desktop app

PurrCode v1.0's graphical product is a **pure-Rust native desktop IDE** (`eframe`/`egui`). It is not a browser portal and not an extension inside somebody else's editor. `purrcode ide` and `purrcode gui` are the same command; neither ever opens a browser.

## Launch

```bash
purrcode ide --repository "$PWD"
```

Or from inside the terminal Workbench:

```text
/ide
```

`purrcode resume --tui` and `purrcode ide` attach to the same daemon session in both directions — you can move between the terminal and the desktop window without losing anything.

The IDE starts or reuses a compatible authenticated local daemon. It never resumes a session implicitly; reopen work with an explicit `--session UUID --repository PATH`.

## What it draws

- an application bar and icon rail;
- session navigation grouped by Today/Yesterday/date, with unread activity marked by a dot;
- a project tree and workspace source-control rail (branch, dirty-file count, ten bounded recent commits);
- a conversation Workbench with a composer and a collapsed, height-bounded Work log;
- a syntax-highlighted editor with tabs, file-type colour icons, a bounded minimap, and cursor position;
- a docked **Diff / Tests / Terminal / Problems / Output** panel.

## Terminal, diff, and validation

- The native terminal uses the daemon's typed PTY routes and the shared ANSI screen buffer: incremental output, input, stop, reconnect-safe offsets, and ownership generations.
- Diff review exposes daemon hunk digests plus Apply/Reject actions.
- Problems and tests are sourced from the daemon's truthful validation artifacts. Missing or skipped evidence renders as unavailable/pending, never as passing.

## Settings

Native settings are grouped around appearance, models/providers, authority, agent behavior, context/skills, terminal/Git, privacy/recovery, and diagnostics. They apply the selected Light/Dark/High-contrast appearance without writing repository state and expose explicit Apply/Reset actions. The IDE never accepts a provider secret — credentials use keychain references only.

## Boundaries

- The IDE owns no session store, no model state, no permission state, and no execution path. All state flows through the daemon.
- All HTTP runs on a worker thread and reaches the UI through channels, so an unreachable daemon reports itself as disconnected instead of freezing the window.
- `purrcode studio` remains a secure browser maintenance/development client. It is not the v1.0 release IDE and nothing routes to it automatically.
