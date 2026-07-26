# PurrCode Provider Qualification Report

Date: 2026-07-25

## Ollama

The local Ollama service was reachable at `127.0.0.1:11434` and advertised
`qwen2.5-coder:7b` among its installed models.

An initial qualification attempt exposed an endpoint construction defect: a configured base URL
ending in `/v1` without a trailing slash was resolved as `/chat/completions`, producing HTTP 404 for
all seven cases. `purrcode-provider-gateway` now normalizes the base path before joining the endpoint,
and a regression test verifies `/v1/chat/completions`.

A second qualification attempt using `purrcode.toml.example` progressed without the 404 response.
It did not complete within 150 seconds and was interrupted rather than left running indefinitely.
No capability passed/failed claims are inferred from an interrupted run.

**Status: INCOMPLETE** — transport routing fixed; model capability and latency qualification still
requires a completed run.

## NVIDIA NIM and LM Studio

No reachable configured instances were available in this environment.

**Status: UNAVAILABLE** — not passed.
