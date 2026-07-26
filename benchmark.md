# PurrCode Benchmark Report

Date: 2026-07-25

## Deterministic catalog

- Catalog audit: 30 tasks, 15 coding fixtures, 10 safety tasks, 7 represented languages.
- Baseline: 22 passed, 0 failed, 8 unavailable.
- The unavailable results are three Go fixtures because Go is not installed and five catalog cases
  without an executable deterministic baseline. They were not counted as successes.

## Live golden benchmark

The earlier two-task run completed daemon infrastructure cleanly but produced 0/2 correct coding
outcomes before its task deadline. The infrastructure 409 lease leak is fixed.

PurrCode now uses the requested whole-task timeout directly (300 seconds by default), rather than
silently applying `min(timeout, fixture maximum_seconds × 10)`. The action prompt also discourages
repeated reads and directs small, known fixes toward a minimal general implementation followed by
validation. Both changes have regression coverage.

A new five-task live run was not started because provider qualification did not complete. This
report does not claim a live pass rate for the revised loop; `benchmark.json` must come from the
next completed live run with a qualified provider.

**Status: BLOCKED ON QUALIFIED PROVIDER**, not an infrastructure failure.
