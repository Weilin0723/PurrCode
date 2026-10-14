# Small-model component result — 2026-10-12

This report preserves the earlier prospectively frozen synthetic trace comparison. The later
repository-snapshot confirmatory study is reported in
[`repository-completion-v1-results.md`](repository-completion-v1-results.md) and is the primary
small-model result. Neither study makes an end-to-end coding-agent efficacy claim.

## Frozen design

Before any v3 model response, the primary reviewer was fixed as `llama3.2:3b`, the fixture was fixed
at 16 paired base-task clusters (one READY and one BLOCKED trace per cluster), and seeds 17 and 29
were declared repeated measurements. The primary test was one two-sided exact sign test at alpha
0.05 after collapsing both traces and both seeds within each base-task cluster. No task, seed, or
response was added, removed, or replaced after execution began. An independent static preflight
found semantic text in four digest values and a missing artifact-revision check; both were corrected
and the fixture was re-frozen before the first model call.

The model-facing prompt contained only request, artifact, and evidence data. Case IDs, pair IDs,
labels, and label rationales remained scorer-only. The deterministic v3 wrapper checked evidence
revision, artifact revision, artifact digest, and exactly one PASS observation for every current
requirement.

## Result

All 64 scheduled calls completed with valid JSON. Each of the 32 traces was run at both seeds; the
two seeds produced identical verdicts for every trace and therefore do not increase the independent
sample size.

| Measure | Deterministic wrapper | Llama 3.2 3B |
| --- | ---: | ---: |
| Trace correctness | 32/32 (100%) | 42/64 calls (65.625%) |
| READY recall | 16/16 traces | 32/32 calls (100%) |
| BLOCKED recall | 16/16 traces | 10/32 calls (31.25%) |
| False-ready | 0 | 22/32 BLOCKED calls |
| Wrong-block | 0 | 0/32 READY calls |

At the prespecified cluster level, the wrapper had 11 wins, zero losses, and five ties. The
two-sided exact sign test is `p=0.0009765625` (1/1024), below the prespecified 0.05 threshold. The
model used 19,754 measured tokens; mean latency was 2,069.71 ms per call and total measured call
latency was 132.46 seconds. The [manifest](evidence/2026-10-12/local-review-v3-llama3.2-3b/manifest.json),
[raw results](evidence/2026-10-12/local-review-v3-llama3.2-3b/results.json),
[summary](evidence/2026-10-12/local-review-v3-llama3.2-3b/summary.json), and
[independent recomputation](evidence/2026-10-12/local-review-v3-llama3.2-3b/audit.json) preserve the
denominators, hashes, dependency hashes, schedule, and limitations.

## Qualification boundary

The same local model was requalified after the legacy Ollama JSON-mode prompt was made explicit.
It improved from the earlier all-schema-failure behavior to 5/7 exact cases, but still failed one
string-typed reasoning response and one patch-string response. It therefore did not pass full
qualification and was recommended only for router/summarizer roles, not coding or high-risk
judgment. The [qualification artifact](evidence/2026-10-12/qualification-llama3.2-3b.json) records
the two failures and the isolated local-only configuration.

## Interpretation

The statistically significant result is narrow: on this authored synthetic completion-trace suite,
an explicit deterministic revision/artifact conjunction follows the rubric more reliably than the
weak 3B reviewer. It is useful evidence that this gate should not be delegated to that model.

It does not establish repository-level coding efficacy, external correctness, user-visible
completion quality, or the planned correct-delivery non-inferiority margin. Labels and wrapper
rules are deliberately closely aligned; the 16 clusters are systematic authored scenarios rather
than a random population sample; and no repository code was executed. The model's 22 false-ready
calls also show that statistical significance must not be read as acceptable reviewer quality.
The subsequent A/B/C repository-snapshot study with an independent executable oracle completed
all 288 scheduled calls. It found a significant false-ready reduction together with a failed
correct-coverage guardrail; see the linked v1 results rather than treating this earlier trace
study as the confirmatory repository result.
