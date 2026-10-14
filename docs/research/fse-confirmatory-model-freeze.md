# FSE confirmatory model and resource freeze

Date: 2026-10-13  
Status: pre-call resource freeze; no E2E outcome is represented here.

This note records the model/resource decision for the 30-task SWE-bench-Live E2E study. It
supersedes the earlier draft that proposed `deepseek-flash` and an 8,192-token session budget.
That draft was never used for an E2E model call.

The detailed reproducible statement lives in
[`swebench-live-v1-model-freeze.md`](swebench-live-v1-model-freeze.md). The frozen decision is:

| Field | Frozen value |
| --- | --- |
| provider | `deepseek` |
| requested model | `deepseek/deepseek-v4-pro` |
| endpoint | `https://api.deepseek.com/` |
| credential reference | `keychain:deepseek`; resolved through the operator's adjacent `credentials.toml`, never copied into the repository |
| fallback | disabled |
| qualification | 7/7 cases, 1,035 ms mean latency, all tested roles recommended |
| measured reliable context | 16,384 tokens |
| scheduled E2E runs | 120 (`30 tasks x B/C x static/correction x repeat 0`) |
| session token budget | 1,000,000 cumulative input+output tokens per run |
| autonomous iteration bound | 32 |
| maximum task wall time | 20 minutes |
| hidden oracle | pinned SWE-bench-Live harness commit `a145aa87c62361532dacaa243398978164b234b7` |
| architecture accommodation | selected public instance images run as `x86_64` |
| Docker credential handling | temporary empty Docker config; no credential helper or keychain is consulted |

The 1,000,000-token session budget was selected before any E2E model call from the runtime's
standard Direct lane budget. The earlier 8,192-token draft was below a normal
plan/implementation/validation/review cycle and would have made the usefulness guardrail an
operational-error study rather than a method comparison.

The model alias is dated service state, not an immutable weight digest. The raw qualification
artifact and the frozen model/readme hashes are bound into the pre-call E2E freeze. No model
substitution, favorable-task replacement, outcome-dependent retry, or optional stopping is
permitted.

This file contains no model-run outcome, p-value, or claim of significance.
