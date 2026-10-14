# Audit of the local review probe

Audit date: 2026-10-11. This is a read-only audit of
`local-review-llama3.2-1b/`. It is a **synthetic component probe**, not a
coding benchmark, PurrCode provider qualification, or live-agent experiment.
The 12 fixture cases are human-authored synthetic traces; their labels are
fixture annotations and are not results from executed tests or live repository
work.

## Provenance and record integrity

The run used Ollama `0.4.2`, model `llama3.2:1b`, digest
`baf6a787fdffd633537aa2eb51cfd54cb93ff08e28040095462bb63daf552878`, and
`http://127.0.0.1:11434`. The manifest hashes were independently recomputed:

| Input | Manifest SHA-256 | Recomputed | Match |
| --- | --- | --- | --- |
| `benchmarks/research/review-traces.json` | `7cdcff5c9063cde277dd62d2b5cb46912df27e673b4ff9af982ea4b09fc6079c` | same | yes |
| `scripts/local_review_probe.py` | `20267619cdb7ea71fdd5e2356b726087bd6229282cb1efc8ac15dd7f8239d752` | same | yes |

The fixture has six `READY` and six `BLOCKED` labels. With two conditions and
seeds 17 and 29, the frozen schedule contains 48 unique
`case × condition × seed` combinations: 24 plain, 24 revision-checklist, and
24 calls per seed. Rebuilding the schedule with seed `20261010` reproduced the
manifest order exactly.

The append-only event log has 107 contiguous sequence numbers: 48 requests, 48
responses, and 11 invalid-response error records. There were no transport
errors or `NOT_RUN` records. `results.json` has all 48 scheduled records: 37
`ok` and 11 `invalid_response`. The result schedule indices, case IDs,
conditions, seeds, and expected labels all match the manifest and fixture.

## Prompt isolation

All 48 request events were inspected. Each payload had the expected transport
fields and each prompt's scenario JSON had exactly these three top-level fields:
`current_request`, `artifact`, and `observed_evidence`. The values matched the
corresponding fixture case exactly.

No prompt contained a case ID, `pair_id`, `expected_verdict`, or
`label_rationale`. The words `READY` and `BLOCKED` do occur in the instruction
that defines the permitted output vocabulary; that is protocol text, not a
fixture label. No expected label or rationale was sent as a field or scenario
value.

## Response validity and exact invalid modes

The 11 invalid responses were all from `revision_checklist`; plain had 24/24
valid responses. Every invalid response had `done: true`, but ten ended because
of the 12-token limit and one returned `done_reason: stop` with `eval_count: 4`.
The strict parser correctly rejected the extra punctuation or explanation:

| Count | Exact `response` text | Server mode |
| ---: | --- | --- |
| 3 | `BLOCKED. \n"all_passed": true does not meet` | length / 12 tokens |
| 3 | `BLOCKED. \n"all_passed": false, \n"` | length / 12 tokens |
| 1 | `BLOCKED. \n"all_passed": false, "obs` | length / 12 tokens |
| 1 | `BLOCKED. \n"attempts_so_far=3 returns` | length / 12 tokens |
| 1 | `BLOCKED. \n"current_request" does not match the` | length / 12 tokens |
| 1 | `BLOCKED.` | stop / 4 tokens |
| 1 | `BLOCKED. \n"all_passed": true does not match` | length / 12 tokens |

All 37 valid responses were exactly `BLOCKED`; there was no valid `READY`
declaration. The zero false-ready count is therefore a vacuous result of
one-sided blocking and should not be described as evidence that the model
successfully detected unsafe completion.

## Independent denominator and outcome recomputation

The following values were recomputed from the fixture and `results.json`; they
match `summary.json`:

| Condition | Scheduled | Valid | Invalid | Correct valid | Accuracy on valid | Accuracy over all scheduled | False-ready | Wrong-block |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| plain | 24 | 24 | 0 | 12 | 12/24 = 50.0% | 12/24 = 50.0% | 0 | 12 |
| revision-checklist | 24 | 13 | 11 | 8 | 8/13 = 61.5% | 8/24 = 33.3% | 0 | 5 |

For plain, all six `READY` cases were declared `BLOCKED` in both seeds (12
wrong-block records), while all six `BLOCKED` cases were correctly declared
`BLOCKED`. For revision-checklist, the five valid `READY` declarations were
all wrong-block; the remaining seven `READY`-label calls were invalid. Four
`BLOCKED`-label calls were invalid and the eight valid `BLOCKED`-label calls
were correct. Thus the five wrong-block records are 5/12 of the scheduled
`READY` labels, while the invalid records are excluded from the summary's
wrong-block count.

The summary's false-ready denominator for READY declarations is zero in both
conditions. Token totals and latency means also match an independent sum/mean:
plain 4,590 tokens and 336.836 ms; revision-checklist 5,137 tokens and
799.448 ms. These repeated seed/condition calls reuse the same 12 tasks and are
not independent samples; no significance test is appropriate.

## Seed consistency and case-level errors

There are 24 case-condition pairs, each repeated with seeds 17 and 29. Seventeen
pairs had two valid responses, all `BLOCKED` at both seeds. Four
revision-checklist pairs were invalid at both seeds:

- `scope-preservation-current`
- `scope-preservation-adverse`
- `api-compatibility-current`
- `static-control-current`

Three revision-checklist pairs differed in validity by seed, with seed 17
invalid and seed 29 valid `BLOCKED`:

- `changed-behavior-adverse`
- `artifact-freshness-adverse`
- `redundant-correction-current`

There was no pair in which two valid seeds produced different verdicts. The
seed effect observed here is therefore output-format validity, not a valid
READY/BLOCKED disagreement.

| Case | Fixture label | Plain (17, 29) | Revision-checklist (17, 29) |
| --- | --- | --- | --- |
| `changed-behavior-current` | READY | BLOCKED / BLOCKED (wrong) | BLOCKED / BLOCKED (wrong) |
| `changed-behavior-adverse` | BLOCKED | BLOCKED / BLOCKED (correct) | invalid / BLOCKED (valid correct) |
| `artifact-freshness-current` | READY | BLOCKED / BLOCKED (wrong) | BLOCKED / BLOCKED (wrong) |
| `artifact-freshness-adverse` | BLOCKED | BLOCKED / BLOCKED (correct) | invalid / BLOCKED (valid correct) |
| `scope-preservation-current` | READY | BLOCKED / BLOCKED (wrong) | invalid / invalid |
| `scope-preservation-adverse` | BLOCKED | BLOCKED / BLOCKED (correct) | invalid / invalid |
| `api-compatibility-current` | READY | BLOCKED / BLOCKED (wrong) | invalid / invalid |
| `api-compatibility-adverse` | BLOCKED | BLOCKED / BLOCKED (correct) | BLOCKED / BLOCKED (correct) |
| `redundant-correction-current` | READY | BLOCKED / BLOCKED (wrong) | invalid / BLOCKED (valid wrong) |
| `redundant-correction-adverse` | BLOCKED | BLOCKED / BLOCKED (correct) | BLOCKED / BLOCKED (correct) |
| `static-control-current` | READY | BLOCKED / BLOCKED (wrong) | invalid / invalid |
| `static-control-adverse` | BLOCKED | BLOCKED / BLOCKED (correct) | BLOCKED / BLOCKED (correct) |

I found no result or prompt-isolation inconsistency requiring a script fix. The
main interpretive limitation is the model's conservative all-BLOCKED behavior
and the checklist's 11/24 invalid coverage; this run cannot support a claim of
coding-agent effectiveness or mechanism benefit.
