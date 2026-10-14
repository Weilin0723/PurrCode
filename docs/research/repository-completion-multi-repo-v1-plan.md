# Multi-repository authored completion suite: protocol expansion

Status: design-only; no model calls or efficacy outcomes are present.

## Purpose

The completed six-task fixture is useful for mechanism work but all six task clusters share one
repository root. This protocol adds the smallest comparable expansion that preserves the existing
144-call shape while making the repository root the independent unit: six independently rooted,
locally authored Rust repositories with one task each.

This is an authored-fixture expansion, not a claim about external open-source repositories. The
suite removes the single-fixture limitation, but it does not by itself establish production or
cross-ecosystem generalization.

## Frozen design before any model call

The complete design is in
`benchmarks/research/multi-repo-v1/suite.json`. It must be hashed and frozen together with the
repository roots, oracle harnesses, preflight, prompt builder, provider runner, and analysis code
before a live run. The existing six-task and DeepSeek v1/v2 artifacts remain immutable.

The suite has six repository roots and six task clusters:

| Repository root | Task | Domain |
| --- | --- | --- |
| `repo-ledger-tax` | `ledger-tax-rounding` | half-up money rounding |
| `repo-ledger-idempotency` | `ledger-idempotent-record` | duplicate-safe recording |
| `repo-settings-timeout` | `settings-timeout-default` | explicit/default timeout semantics |
| `repo-settings-headers` | `settings-header-merge` | base/override header preservation |
| `repo-scheduler-backoff` | `scheduler-bounded-backoff` | capped exponential retry delay |
| `repo-scheduler-order` | `scheduler-priority-order` | priority and stable ordering |

Each root has its own Cargo manifest, source modules, public tests, and behavior oracle. The
candidate set, two prompt conditions, and replicate labels `17` and `29` are unchanged from the
existing completion protocol. `gold_stale_evidence` reuses the gold source mutation but receives
stale evidence metadata; it is externally accepted by the behavior/preservation contract and
blocked by the binding gate.

The schedule is:

```text
6 repository clusters × 1 task × 6 candidates × 2 conditions × 2 labels = 144 calls/model
```

The primary condition remains `revision_rubric_json`. The sole primary estimand remains the mean
cluster difference in externally rejected false-ready risk, `C - B`, with the repository root as
the cluster. Candidate rows and replicate labels are not independent clusters.

## Preflight and validity

Run the offline preflight before any provider qualification or model call:

```text
python3 benchmarks/research/multi-repo-v1/preflight.py
```

It must verify all six roots, apply each mutation in a temporary worktree, run public Cargo tests,
run an isolated Rust behavior oracle, and check unchanged paths and test integrity. A preflight
pass is only fixture validation; it is not a live result and must not be reported as model
efficacy.

A future live runner must preserve the existing rules: complete the schedule, count invalid,
timeout, provider-error, and missing-terminal rows in ITT attrition, suppress inference when the
predeclared operational threshold is exceeded, and never replace an uncertain or failed call based
on its observed verdict. It must not reuse rows from the existing single-fixture or DeepSeek runs.

The five-percentage-point correct-coverage guardrail remains unchanged and must show `gold` and
`gold_stale_evidence` separately. A gate-induced loss on stale evidence is a safety--availability
diagnostic; it cannot be silently relabeled as a model failure or removed to make the guardrail
pass.

## Claims allowed after a future valid run

At most, a valid result supports a narrow statement about the frozen six-root authored suite and
the dated model alias/protocol used. It does not support claims of semantic correctness beyond the
independent fixture oracles, end-to-end coding-agent efficacy, open-source repository performance,
or general FSE/ICLR method superiority. The suite remains a follow-up expansion and must not be
pooled with earlier results to manufacture a single significance claim.
