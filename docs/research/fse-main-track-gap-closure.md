# FSE main-track gap closure status

Date: 2026-10-13

This note tracks the four reviewer-facing gaps against concrete repository artifacts. It deliberately
separates *implemented and frozen* from *empirically passed*. A run that has not happened is not
described as evidence.

| Gap | Current closure | Evidence | Remaining gate |
| --- | --- | --- | --- |
| Only 6 authored tasks | Closed in the design: 30 tasks across 10 public repositories, three tasks per repository cluster | `benchmarks/research/swebench-live-v1/manifest.json`, `run-bundle.json` | Execute the frozen 120 cells on x86_64 |
| Single fixture | Closed in the design: ten independent upstream checkouts at pinned base commits | acquisition manifest and checkout preflight | Execute and analyze the cross-repository schedule |
| Non-end-to-end | Closed in the runner: real daemon session, real repository edits through PawGate/Claw, two-phase P1 delivery, final patch graded by the pinned hidden oracle | `scripts/swebench_live_e2e.py`, synthetic correction smoke, `.github/workflows/research-swebench-live.yml` | Produce complete 120-row result after the x86_64 gate |
| Guardrail failure | Reopened as a hypothesis, not hidden: arm C uses the live review gate and re-reviews the unchanged artifact after the P1 revision; the guardrail is measured on all-scheduled correct delivery | `analysis-plan.json`, `freeze.json`, runner correction protocol | Pass the frozen non-inferiority criterion or report the remaining trade-off |

## Why the final run is delegated to x86_64 CI

The selected SWE-bench-Live images are published as `x86_64`. On the local Apple Silicon host,
Rosetta can execute a gold patch but native extension imports eventually terminate the test process
with `Illegal instruction`. That is an infrastructure failure, not an oracle label. The final
workflow therefore runs on `ubuntu-24.04`, pulls the x86_64 images natively, and performs the gold
preflight before any scheduled E2E cell.

## What is already frozen

- public dataset revision and all 30 base commits;
- the controlled P0/P1 revision and its hashes;
- the 120-cell B/C x static/correction schedule;
- the model/resource freeze, including the 1,000,000-token session budget;
- the runner/oracle source hashes and run-bundle hash;
- the analysis plan, freeze, and independent receipt digest.

The only missing input for the full run is the repository secret `DEEPSEEK_API_KEY` on the GitHub
Actions runner. The secret is consumed only to materialize an operator-side `credentials.toml`;
the daemon's stored reference remains `keychain:deepseek`, and no credential value is written to
the repository, model context, events, or tool processes.
