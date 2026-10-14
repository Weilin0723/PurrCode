# DeepSeek V4 Pro repository-completion operational-repair plan

Date: 2026-10-13  
Study ID: `repository-completion-deepseek-v2`  
Status: pre-call plan for an outcome-aware operational repair

## Why a second run is necessary

The completed v1 schedule produced a terminal record for all 144 calls, but 26 responses ended at
the frozen 128-token ceiling and failed JSON parsing. Its 18.1% attrition exceeded the predeclared
10% operational-validity limit, so v1 is permanently classified as operationally inconclusive.
The v1 artifact and its descriptive outcomes remain immutable and are not pooled with this run.

This v2 run repairs only that observed transport/output defect. Because the v1 outcomes are already
known, v2 is an outcome-aware follow-up, not an independent preregistration or a fresh confirmation.
The decision to run v2 is based on ceiling-linked JSON truncation, not on whether v1's effect was
favorable or significant.

## Frozen change and invariants

The sole protocol change is:

- `max_output_tokens`: 128 -> 256.

Everything else remains fixed from v1:

- official DeepSeek API and dated `deepseek-v4-pro` alias;
- schema-constrained, non-thinking structured output through the Rust provider gateway;
- the same six tasks, 36 candidate snapshots, two prompt conditions, and replicate labels 17/29;
- the same 144-call order and fresh execution of every call;
- the same external oracles, B/C/D definitions, primary condition, ITT handling, inference,
  guardrail, and secondary analyses;
- no API seed claim, no replacement of errors, and no optional stopping.

No v1 response is reused. The v2 request bundle, plan, scripts, runner, dependencies, and hashes
must be frozen before call 0. Credentials remain outside prompts, argv, environments, events, and
artifacts.

## Analysis and operational validity

The sole primary estimand remains the six-task-cluster mean difference in externally rejected
false-ready rates under `revision_rubric_json`:

```text
Delta_DS_v2 = mean_task(false_ready_C_task - false_ready_B_task)
```

If and only if all 144 indices have terminal records, failed/invalid/missing attrition is at most
10%, and provider usage is sufficiently complete, report the original two-sided exact paired sign
test at alpha 0.05, the 10,000-replicate paired cluster bootstrap interval using seed 20261013, and
the descriptive 64-assignment sign-flip null-reference interval. Otherwise the efficacy
interpretation is operationally inconclusive and inferential values remain suppressed.

The unchanged usefulness guardrail requires correct-coverage C-minus-B to be no worse than -5
percentage points across `gold` and `gold_stale_evidence`, also reported separately. Raising the
output ceiling cannot repair a gate-induced coverage loss. A significant false-ready reduction
with a failed guardrail remains a safety--availability trade-off, not a method win.

## Budget and stopping

The hard budget is 150,000 provider-reported input plus output tokens. With v1's observed 93,468
input tokens as a planning value, 144 calls at the full 256-token ceiling would total at most
130,332 tokens. Stop only for the hard token ceiling, billing/usage exhaustion, provider outage,
credential or integrity failure, or user interruption. Resume only never-started indices from the
identical freeze. Do not alter the cap again within v2.

## Claim boundary

Even a valid and significant v2 result is limited to an operationally repaired replication on one
authored six-task fixture using a dated API alias. It is not cross-repository generalization,
semantic correctness, end-to-end coding-agent efficacy, or an unconditional FSE/ICLR method win.
The paper must identify v2 as an outcome-aware repair and keep v1 visible.

