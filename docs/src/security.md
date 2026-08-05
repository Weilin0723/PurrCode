<p align="center">
  <a href="README.md"><img src="purrcode-logo.png" alt="PurrCode" width="180"></a>
</p>

# Security

PurrCode assumes the following may be untrusted:

- model output;
- repository content;
- downloaded skills;
- MCP servers;
- web content;
- imported configuration;
- generated commands;
- tool output.

## Authorization binding

PawGate authorizes the exact serialized action and constraints. Claw verifies the authorization again immediately before execution.

Authorizations are:

- durable;
- digest-bound;
- single-use;
- consumed atomically.

A model can never create, widen, re-scope, impersonate, escalate, or hide a grant.

## Process execution

PurrCode uses explicit argument vectors rather than shell command strings. Execution environments are scrubbed of credentials unless an explicitly authorized capability receives a secret reference.

## Repository isolation

Agent work runs in detached Git worktrees under PurrCode-managed storage. Existing uncommitted work is not silently stashed, overwritten, discarded, or copied into the isolated worktree.

## Sandbox support

PurrCode uses available host isolation:

- macOS: `sandbox-exec` when available;
- Linux: Bubblewrap when available;
- Windows: process filtering and job controls.

Inspect the current host environment:

```bash
purrcode sandbox doctor
```

Degraded isolation is reported explicitly and is not represented as a full sandbox.

## Secret protection

Provider keys and secret-like values are:

- detected before durable conversation persistence;
- redacted from diagnostic excerpts;
- stored using keychain or environment references;
- scrubbed from tool and plugin child environments;
- excluded from imported-source review output.

## Security documentation

Read the repository's [security model](https://github.com/Weilin0723/PurrCode/blob/main/docs/security.md), [architecture](https://github.com/Weilin0723/PurrCode/blob/main/docs/architecture.md), and [production acceptance audit](https://github.com/Weilin0723/PurrCode/blob/main/docs/production-acceptance.md) before using PurrCode for sensitive repositories.
