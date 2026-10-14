# Local benchmark results — 2026-10-11

This report freezes the local-only evidence collected for the FSE working draft. It separates
three different measurements so that a provider qualification, a synthetic review probe, and a
coding-agent benchmark are not conflated.

## 1. Deterministic mechanism benchmark

After implementing requirement-revision and artifact-digest binding, all six hand-authored
revision scenarios are supported and pass. The reducer itself produced these outcomes; the
benchmark does not substitute model labels for runtime state.

| Scenario | Expected | Observed | Result |
| --- | --- | --- | --- |
| Correct fresh evidence | Ready | Ready | pass |
| Stale evidence after revision | PartiallyComplete | PartiallyComplete | pass |
| Artifact edited after verification | PartiallyComplete | PartiallyComplete | pass |
| Scope preservation violation | Blocked | Blocked | pass |
| Redundant/no-op correction | Ready | Ready | pass |
| Benign static control | Ready | Ready | pass |

The intervention binds evidence to the last revision that changed its requirement and to the
repository patch digest reviewed. Unrelated contract edits do not stale evidence; a semantic
requirement change or later artifact change does. The 6/6 result has a wide Wilson 95% interval
for a pass proportion (60.97%–100%) and is a deterministic regression result, not a population
estimate. See the [machine-readable v2 result](evidence/2026-10-11/revision-mechanism-v2.json).

## 2. Initial synthetic local-model review probe

The local probe used Ollama 0.4.2 and `llama3.2:1b` (digest
`baf6a787fdffd633537aa2eb51cfd54cb93ff08e28040095462bb63daf552878`) on 12
human-authored synthetic traces, two prompt conditions, and seeds 17 and 29. The 48 calls are
repeated measurements of 12 tasks and are not independent task samples.

| Condition | Scheduled / valid | Correct among valid | Correct among scheduled | Wrong-block among expected READY | Invalid | Tokens | Mean latency |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| Plain | 24 / 24 | 12/24 (50%) | 12/24 (50%) | 12/12 | 0 | 4,590 | 336.84 ms |
| Revision checklist | 24 / 13 | 8/13 (61.54%) | 8/24 (33.33%) | 5/12 | 11 | 5,137 | 799.45 ms |

All 37 valid responses were `BLOCKED`; there were no valid `READY` declarations. The apparent
zero false-ready count therefore comes from never declaring readiness and does not show useful
completion judgment. Ten invalid checklist responses were truncated at the 12-token limit, and
one returned `BLOCKED.` rather than the required exact label. The checklist increased latency and
invalid output without establishing efficacy.

The frozen [manifest](evidence/2026-10-11/local-review-llama3.2-1b/manifest.json),
[summary](evidence/2026-10-11/local-review-llama3.2-1b/summary.json), and
[independent Luna audit](evidence/2026-10-11/local-review-audit.md) contain the complete evidence.
This is a synthetic component probe with no code execution or repository-level correctness claim.

## 3. Frozen v2 intervention benchmark

The v2 fixture was frozen before the deterministic gate was run. It contains 12 paired synthetic
traces: six current, fully evidenced artifacts and six adverse variants covering stale requirement
revision, stale artifact digest, failed preservation, missing evidence, and unknown evidence.
Each model condition used six paired base-task clusters (12 traces) with seeds 17 and 29, so the
24 calls are repeated measurements rather than 24 independent tasks.

| Judge | Format coverage | Accuracy | READY recall | BLOCKED recall | False-ready | Wrong-block |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Llama 3.2 1B, revision rubric | 24/24 (100%) | 12/24 (50%) | 0/12 (0%) | 12/12 (100%) | 0 | 12 |
| Llama 3.2 3B, revision rubric | 24/24 (100%) | 16/24 (66.67%) | 12/12 (100%) | 4/12 (33.33%) | 8 | 0 |
| Deterministic revision/artifact gate | 12/12 (100%) | 12/12 (100%) | 6/6 (100%) | 6/6 (100%) | 0 | 0 |

The intervention passed its prespecified engineering thresholds on this development fixture; the
1B and 3B rubric conditions did not. The deterministic result's Wilson 95% interval is
75.75%–100%, which shows how little 12 authored traces establish about generalization.

Collapsing both seeds and both traces within each of the six paired base-task clusters, the exact
two-sided sign comparison is 6 wins, 0 losses, 0 ties versus the 1B rubric (`p=0.03125`) and
4 wins, 0 losses, 2 ties versus the 3B rubric (`p=0.125`). Treating the two local-model comparisons
as one family gives Holm-adjusted values
of 0.0625 and 0.125. Therefore the observed development-set effect is large and operationally
useful on this frozen fixture, but it is not family-wise significant at 0.05 and is not a
confirmatory efficacy claim.
Counting the two seeds as 24 independent tasks would be pseudoreplication and is not reported.

Artifacts: [1B summary](evidence/2026-10-11/local-review-v2-llama3.2-1b/summary.json),
[3B summary](evidence/2026-10-11/local-review-v2-llama3.2-3b/summary.json), and
[deterministic gate summary](evidence/2026-10-11/deterministic-review-gate-v2/summary.json).

## 4. PurrCode provider qualification

The same model failed the existing PurrCode provider qualification: 0/7 cases passed, the command
exited 1, and every case omitted the required `answer` field. `recommended_roles` was empty. The
qualification used a pre-existing binary rather than a fresh build of the current repository head;
its hash and timestamp are recorded in the
[qualification artifact](evidence/2026-10-11/qualification-llama3.2-1b.json).

The repository qualification gate therefore prevented a live coding-agent benchmark with this
model. No repository coding success rate, external false-completion rate, or API-model comparison
is claimed in this local result set.

## 5. Validation

- `cargo fmt --check`: passed.
- `cargo clippy --workspace --all-targets -- -D warnings`: passed.
- `CARGO_INCREMENTAL=0 cargo test --workspace -j 1`: passed after rebuilding the CLI prerequisite. The first attempt reached the TUI E2E suite but its 10 accessibility cases refused to run because `target/debug/purrcode` was older than `crates/purrcode-cli/src/main.rs`; `CARGO_INCREMENTAL=0 cargo build -p purrcode-cli -j 1` refreshed the documented prerequisite before the complete rerun. The rerun executed all 10 accessibility cases successfully.
- `cargo check -p purrcode-evaluation-runtime --features benchmark --bin purrcode-alignment-benchmark -j 1`: passed in an isolated target directory.
- `cargo test -p purrcode-evaluation-runtime --features benchmark --bin purrcode-alignment-benchmark -j 1`: 6 passed, 0 failed in a fresh isolated target directory.
- `cargo test -p purrcode-runtime-core --lib -j 1`: 286 passed, 0 failed as part of the full workspace run.
- `cargo test -p purrcode-runtime-core --test revision_benchmark -j 1 -- --nocapture`: passed and wrote the six-scenario result.
- `CARGO_INCREMENTAL=0 cargo check -p purrcode-skill-store -p purrcode-skill-registry -p purrcode-web-research -j 1`: passed.
- `python3 -S -m unittest scripts.test_deterministic_review_gate_v2 scripts.test_local_review_probe_v2 scripts.test_local_review_probe scripts.test_research_metrics`: 16 passed, 0 failed. `-S` avoids a site-package named `scripts` shadowing this repository's namespace package.
- `git diff --check`: passed.

The API large/small-model comparison and the 36-run repository coding pilot remain future runs.
