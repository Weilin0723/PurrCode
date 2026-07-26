# PurrCode Provider Qualification Report

Date: 2026-07-26

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

## NVIDIA NIM and LM Studio

No reachable configured instances were available in this environment.

**Status: UNAVAILABLE** — not passed.
