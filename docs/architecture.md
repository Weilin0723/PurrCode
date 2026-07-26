# PurrCode architecture

PurrCode is organized as four cooperating subsystems: **PawGate** owns judgment and authorization,
**Claw** owns controlled tool execution and sandboxing, **Whisker** owns repository context and risk
signals, and **NineLives** owns durable checkpoints, recovery, and rollback.

The first vertical slice deliberately keeps model providers outside the trusted decision path.

```text
ProposedAction
  -> deterministic Policy
  -> JudgmentDecision + constraints
  -> append-only SQLite authorization
  -> exact action/constraint digest verification
  -> atomic single-use authorization consumption
  -> shell-free process execution
  -> validation event
```

## Trust boundaries

- `runtime-core` owns serializable provider-independent domain types.
- `judgment-engine` is deterministic and has no model or network dependency.
- `session-store` owns durable events and single-use authorizations.
- `tool-runtime` must independently verify authorization before spawning.
- `provider-gateway` defines model contracts but cannot authorize tools.
- `purrcode-cli` composes these crates; it is not itself a trust boundary.

The current process backend scrubs credentials and uses an explicit argument vector. It does not
yet provide OS-level filesystem or network isolation, so policy only auto-authorizes command forms
whose normal behavior is read-only (`git status/diff/show/log/rev-parse/ls-files` and `rg` without a
preprocessor). This limitation is displayed in the implementation status and is not described as a
full sandbox.
