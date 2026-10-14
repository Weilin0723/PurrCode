# PurrCode Provider Qualification Report

Date: 2026-08-12; updated 2026-10-13

## DeepSeek

Two OpenAI-compatible remote DeepSeek routes were configured without local-model or remote-fallback
paths:

- `deepseek-v4-pro`, used for planning, coding, and repair; and
- `deepseek-v4-flash`, used for routing and independent reviews.

Real `purrcode model qualify` runs completed all seven qualification cases for each configured model.
Both models scored 7/7 and were accepted for their assigned roles. The qualification covered
structured output, context retention, tool schema use, multi-file reasoning, patch generation,
test-failure interpretation, and judgment calibration.

Later alignment dogfooding exercised both routes across repeated live tasks. After the qualified
runs and several successful delivery-gated tasks, the provider began returning HTTP 402
`Insufficient Balance`. That is recorded as an account-availability failure and prevented the final
catalog rerun; it is not rewritten as either a model qualification failure or a successful
benchmark.

On 2026-10-13, a newly configured official `deepseek-v4-pro` route again completed all seven cases
at 7/7 with 1,136 ms mean latency and was recommended for every tested role, including high-risk
judgment. The machine-readable result is
[`deepseek-v4-pro-qualification.json`](../research/evidence/2026-10-13/deepseek-v4-pro-qualification.json).
The route then completed the repository-completion research schedules, confirming that the earlier
402 was a historical account-availability incident rather than the current state. This fresh run
does not requalify or make a current catalog claim for the older flash alias.

**Updated status: `deepseek-v4-pro` QUALIFIED 7/7 AND REACHABLE; THE EARLIER 402 REMAINS HISTORICAL
EVIDENCE.**

## Ollama

The local Ollama service was reachable at `127.0.0.1:11434` and advertised
`qwen2.5-coder:7b` among its installed models.

An initial qualification attempt exposed an endpoint construction defect: a configured base URL
ending in `/v1` without a trailing slash was resolved as `/chat/completions`, producing HTTP 404 for
all seven cases. `purrcode-provider-gateway` now normalizes the base path before joining the endpoint,
and a regression test verifies `/v1/chat/completions`.

A real daemon `/connect` smoke test and provider-backed multi-turn streaming test completed against
the running service. The complete `purrcode model qualify ollama/qwen2.5-coder:7b` suite then ran to
completion: structured output and context retention passed; tool schema, multi-file reasoning,
patch generation, test-failure interpretation, and judgment calibration failed because the model
omitted the required `answer` field. Accuracy was 2/7 (28.57%), mean latency 11,808 ms, and no role
was recommended.

**Status: FAILED QUALIFICATION** — connectivity and multi-turn streaming are verified, but this
installed model is not release-qualified. The failure is preserved rather than represented as a
pass.

On 2026-10-12, a local-only `llama3.2:3b` run identified a second interoperability issue: Ollama
0.4.2 supports JSON mode but not schema-valued `format`, while the fallback qualification prompt
named the `answer` field without stating its exact JSON shape. The prompt now specifies one
`{"answer":"<requested answer>"}` object without relaxing exact-match scoring. A fresh CLI build
then completed the suite at 5/7. Structured output, tool schema, context retention, test-failure
interpretation, and judgment calibration passed; multi-file reasoning returned numeric `42` where
the schema required a string, and patch generation included an extra `}`. The model was recommended
only for router/summarizer roles and remains unqualified for coding or high-risk judgment. The
[machine-readable result](../research/evidence/2026-10-12/qualification-llama3.2-3b.json) preserves
the failures, binary/source hashes, local-only configuration, and empty credential environment.

**Updated status: FAILED QUALIFICATION (5/7)** — JSON compatibility improved, but the remaining
schema and exact-output failures are real qualification failures.

The same-day local-only `llama3.2:1b` snapshot passed 3/7 checks and exited nonzero. It failed
multi-file reasoning, patch generation, context retention, and test-failure interpretation; no
role was recommended. The run used Ollama 0.4.2 with an empty credential environment, but the
repository was dirty, so it is evidence about the study model rather than a clean-release
qualification. The sanitized [feasibility summary](../research/small-model-feasibility-2026-10-12.md)
and its machine-readable companion preserve the model, binary, and provider-source digests.

**1B status: FAILED QUALIFICATION (3/7)** — retained as a secondary research model only.

## NVIDIA NIM and LM Studio

No reachable configured instances were available in this environment.

**Status: UNAVAILABLE** — not passed.
