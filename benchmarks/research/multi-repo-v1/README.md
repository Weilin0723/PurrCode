# Multi-repository authored completion suite

This directory is a design-only expansion of the repository-completion benchmark. It contains
six separate, locally authored Rust repository roots, each with one bounded task, an independent
public test surface, and an isolated behavior oracle. The repositories are intentionally tiny so
that the suite can be audited before any model is contacted.

The suite is not evidence from a live model. `suite.json` contains task specifications and
candidate mutation recipes only; no response, efficacy, or significance result is present. Run
the offline preflight with:

```text
python3 benchmarks/research/multi-repo-v1/preflight.py
```

The preflight creates temporary candidate worktrees, runs public Cargo tests, invokes the external
Rust behavior oracle, and checks unchanged-path and test-integrity contracts. It reports zero model
calls and does not write a result artifact.

## Design

The six roots cover two ledger tasks, two settings tasks, and two scheduler tasks:

- half-up tax rounding;
- duplicate-safe identifier recording;
- default timeout selection;
- preserving and overriding header maps;
- bounded exponential retry backoff; and
- descending-priority stable queue ordering.

Each task has the same six candidate roles used by the existing study: `baseline`, `gold`,
`stale`, `overbroad`, `test_tampering`, and `gold_stale_evidence`. The latter reuses the gold
artifact with stale evidence metadata so the deterministic binding gate's availability cost stays
visible. Candidate labels are not sent to a model.

The proposed schedule is 6 repositories × 1 task × 6 candidates × 2 conditions × 2 replicate
labels = 144 calls per model. The repository root, not a candidate row or replicate label, is the
inferential cluster. This preserves six clusters while removing the single-fixture limitation.
The suite still has important limits: the roots are synthetic and locally authored, each root has
only one task, and this is not evidence of open-source or production-repository generalization.

No existing six-task fixture, DeepSeek v1/v2 artifact, or small-model result is modified or pooled
by this directory. A future live run needs its own preregistration/freeze, dependency hashes,
credential-safe runner, and complete terminal-event audit before inference is enabled.
