<p align="center">
  <a href="README.md"><img src="purrcode-logo.png" alt="PurrCode" width="180"></a>
</p>

# Modes

PurrCode separates two kinds of modes: **task modes** and **permission modes**. Both are shown in the header and travel with the session, so a read-only mode is a constraint the daemon enforces rather than a hint.

## Task modes

`Ctrl+K` / `/mode` switches between:

| Mode | Meaning |
|---|---|
| **Ask** | Conversation and read-only inspection. Nothing is written. |
| **Plan** | Produces a plan and pauses. The plan stays open to a reply — say what to change and it is rewritten as a numbered revision and paused again, for as many rounds as you need. Nothing is written to disk in any of them. |
| **Build** | Carries out the settled plan: implements, tests, and validates. |
| **Review** | Reviews the current work and diff. |

To start executing a plan you settled on, use `Build this plan` or `/resume` in the same session.

## Permission modes

`/permission` switches between:

| Mode | Meaning |
|---|---|
| **Ask** | Every proposed action requires explicit human approval. |
| **Auto** | Actions authorized by durable policy proceed without interrupting for each one. |
| **Full Access** | Uses the full authority the process already holds. Grants nothing the process does not already have, and the UI says so. |

Full Access does not grant any permission the process does not already hold. Read-only modes refuse mutation; the daemon enforces the boundary.

## Adaptive workflow

New sessions use a daemon-resolved `auto` task mode. Based on task evidence, the daemon selects a workflow profile:

| Profile | Meaning |
|---|---|
| **Direct** | Single specialist lane for straightforward work. |
| **Standard** | Balanced plan/build/test flow. |
| **Ultra** | Bounded parallel specialist lanes for complex work. |

The TUI header also shows the current workflow and search policy; `/usage` exposes the recorded token/cost ledger.
