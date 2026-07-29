# PurrCode benchmark status

Date: 2026-07-29

## Deterministic catalog

- Catalog audit: 30 tasks, 15 coding fixtures, 10 safety tasks, 7 represented languages.
- Baseline: 22 passed, 0 failed, 8 unavailable.
- The unavailable results are three Go fixtures because Go is not installed and five catalog cases
  without an executable deterministic baseline. They are not counted as successes.

## Live golden benchmark

No five-task v0.6.0 live benchmark is represented as successful. A qualified provider and the
resulting `benchmark.json` are required before this external gate can pass. The earlier two-task
run completed daemon infrastructure cleanly but produced 0/2 correct coding outcomes before its
deadline; it is historical failure evidence, not release qualification.

The runtime now honors the requested whole-task deadline directly (300 seconds by default), and
typed repository reads reduce ambiguous or repeated shell-shaped exploration. Those changes have
regression coverage, but local deterministic tests are not substituted for live provider evidence.

**Status: EXTERNAL GATE — qualified provider required.**
