# SWE-bench-Live E2E model and resource freeze

Date: 2026-10-13  
Status: pre-call model/resource freeze; no E2E outcome is represented here.

This note fixes the model and runtime resources for the 30-task real-repository E2E study. It does
not change the 120-cell schedule or the primary estimand in the
[confirmatory plan](repository-completion-swebench-live-v1-plan.md).

## Frozen model

| Field | Frozen value |
| --- | --- |
| provider | `deepseek` |
| requested model | `deepseek/deepseek-v4-pro` |
| endpoint | `https://api.deepseek.com/` |
| API shape | OpenAI-compatible chat/completions |
| credential reference | `keychain:deepseek`, resolved to the operator's adjacent `credentials.toml`; the secret is never copied into this repository |
| fallback | disabled |
| provider qualification | 7/7 cases, 1,035 ms mean latency, all tested roles recommended |
| measured reliable context | 16,384 tokens |
| model/API version | provider alias at the dated run; not an immutable weight digest |

The raw qualification output is stored outside the public repository and its SHA-256 is bound into
the E2E pre-call freeze. The provider alias is dated service state, not a weight digest.

## Frozen run resources

| Field | Frozen value |
| --- | --- |
| scheduled E2E runs | 120 (`30 tasks x B/C x static/correction x repeat 0`) |
| session token budget | 120,000 cumulative input+output tokens per run |
| autonomous iteration bound | 32 |
| maximum task wall time | 20 minutes |
| hidden oracle timeout | 30 minutes per scored run |
| oracle | pinned official SWE-bench-Live harness, commit `a145aa87c62361532dacaa243398978164b234b7` |
| architecture accommodation | selected remote images run as `x86_64` |
| Docker credentials | temporary empty Docker config; no credential helper or keychain is consulted |

The earlier `8,192`-token draft budget was replaced before any E2E model call because it is below
the cost of a normal plan/implementation/validation/review cycle and would turn the usefulness
guardrail into an operational error study. The 120,000-token value is the frozen protocol input,
not an observed result.

## Stop, retry, and resume rules

1. Publish the model/resource freeze, analysis plan, schedule, run bundle, and artifact hashes
   before scheduled run 0.
2. Execute the scheduled run IDs in the frozen order. A run may make multiple provider calls; all
   calls remain in the run-level token ledger.
3. Never replace a task, arm, variant, model, or failed run based on an interim outcome.
4. A provider/transport failure produces an ITT `error` row. A task deadline produces a `timeout`
   row. Neither is rerun as a replacement.
5. Resume only never-started or non-terminal scheduled runs from the identical bundle and freeze.
6. If the frozen attrition gate is breached, the analysis must suppress method support rather
   than deleting failed rows.
