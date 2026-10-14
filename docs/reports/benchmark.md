# PurrCode benchmark status

Date: 2026-08-12

## Deterministic catalogs

- The original golden catalog audit remains 30 tasks, 15 coding fixtures, 10 safety tasks, and 7
  represented languages. Its deterministic baseline remains 22 passed, 0 failed, and 8 unavailable;
  unavailable cases are not counted as successes.
- The v1.5 alignment catalog contains 20 tasks whose repository outcomes are checked independently
  of the model and whose delivery state is derived from the durable session log.
- Scripted alignment measurement tests pass for the satisfied, violated, and corrected cases. These
  tests verify the harness and reducer; they are not substituted for live provider evidence.

## Live v1.5 alignment run

Dogfooding used only the configured remote DeepSeek routes. No local model or remote fallback was
enabled. Both configured models had first completed the seven-case provider qualification suite.

Focused live tasks reached `gate=ready` with their independent fixture checks passing, including:

- `off-by-one`: 2/2 checks;
- `config-default`: 2/2 checks;
- `add-one-test`: 2/2 checks; and
- `paginate-endpoint`: 3/3 checks.

The focused runs and interrupted catalog attempts exposed real recovery defects in structured
response normalization, fresh-read enforcement, task attribution, validation routing, blocking
question provenance, and pre-plan discovery evidence. Each observed defect was fixed with focused
regression coverage before being rerun.

The latest coherent catalog attempt produced four consecutive `gate=ready` tasks (`rename-constant`,
`off-by-one`, `config-default`, and `add-one-test`). `button-label` then made the correct change and
passed 2/2 independent checks, but stopped before its delivery gate because a discovery-only plan
step was recorded after the successful read that satisfied it. That lifecycle defect was fixed and
covered, but the next live rerun received HTTP 402 `Insufficient Balance` from the provider before
the intent compiler or coding worker could proceed.

No partial run is archived as `benchmark.json`, and no interrupted result is represented as a pass.
The final report will replace this status only after one coherent 20-task run satisfies the release
bar and the native IDE dogfood walkthrough is recorded.

**Status: EXTERNAL GATE — remote provider balance required to finish the live run.**
