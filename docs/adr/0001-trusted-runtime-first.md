# ADR 0001: Build the trusted runtime before model adapters

Status: accepted

The product specification spans daemon, TUI, IDE, providers, indexing, worktrees, sandboxing, MCP,
and SDKs. Implementing those surfaces before authorization semantics would multiply unsafe paths.

The first milestone therefore establishes typed actions, deterministic judgment, exact
authorization binding, append-only persistence, at-most-once consumption, and bounded execution.
Model adapters may propose these actions later but cannot bypass this path.

SQLite is embedded with WAL and foreign keys. Authorization and its audit event are committed in
one transaction. Consumption is an immediate transaction so concurrent or restarted callers cannot
execute the same authorization twice.

