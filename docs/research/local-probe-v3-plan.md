# Frozen v3 component-study plan

Frozen before model execution on 2026-10-12. An independent static preflight found semantic text
in four adverse digests and a missing artifact-revision check. No v3 model call had been made.
Those two defects were removed, tests were added, and the fixture was re-frozen before execution.

## Claim boundary

This study tests one narrow question: on new synthetic completion traces, does the deterministic
revision/artifact gate classify paired current/adverse evidence more reliably than the configured
local reviewer? It does not execute repositories and cannot establish coding-agent efficacy,
external correctness, or the planned correct-delivery non-inferiority claim.

## Primary analysis

- Primary reviewer: `llama3.2:3b`, the configured local reviewer.
- Fixture: 16 new base-task clusters, each containing one READY and one BLOCKED trace.
- Repeats: seeds 17 and 29. Repeats are not independent.
- Cluster score: deterministic correctness over the two traces versus model scheduled correctness
  over the same two traces and both seeds; invalid model output counts as incorrect.
- Test: one two-sided exact sign test over the 16 paired base-task clusters, alpha 0.05.
- Decision: significant only when the primary-model p-value is below 0.05. No optional stopping,
  task deletion, seed substitution, or post-result fixture edit is allowed.

The development result had four discordant wins and two ties across six clusters for this model.
Using its observed discordance rate only for planning, 11 new clusters give at least 80% probability
of observing the six discordant wins required for a two-sided sign-test p-value of 0.03125 when the
gate has no losses. Sixteen clusters provide margin for lower discordance. The fixture remains a
systematic authored suite rather than a random population sample, so any significant result is
reported as component evidence with limited external validity.

Results are written once to a new evidence directory with fixture, script, model, schedule, and
prompt hashes. A later 1B run, if any, is secondary and does not alter the primary decision.
