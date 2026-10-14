# SWE-bench-Live real-repository expansion plan

**Status:** acquisition pending; no model calls or outcomes are represented here.

## Purpose and claim boundary

The completed six-task fixture and its DeepSeek v1/v2 runs are useful for mechanism and transport
audits, but they do not support a claim about independent open-source repositories. The authored
six-root expansion removes a single-fixture limitation for offline testing only; it is synthetic
and cannot close external validity. This plan adds a frozen acquisition path for real
SWE-bench-Live tasks without fabricating candidate or model outcomes.

The eventual result, if the full protocol is completed, remains limited to the selected dated
SWE-bench-Live snapshot, task construction, oracle, model, and prompt. It cannot be presented as
general software-engineering performance, semantic correctness beyond the audited oracle, or an
unconditional FSE/ICLR method win. The current artifact is not evidence of significance.

## Frozen source and task set

The source is the `verified` split of `SWE-bench-Live/SWE-bench-Live`, configuration `default`,
revision `b51a86422e10cfd403beb4773e5a2947953e36ec`. The complete machine-readable freeze is
`benchmarks/research/swebench-live-v1/manifest.json`; its schema is
`benchmarks/research/swebench-live-v1/schema.json`.

Selection scans all 500 rows in `row_idx` order, marks rows with at most 50 changed lines and
non-empty `FAIL_TO_PASS` and `PASS_TO_PASS` sets as eligible, orders repository groups by their
first eligible row, and retains the first ten groups with at least three eligible rows. The first
three eligible rows from each group are frozen. The scan contains 370 eligible rows across 87
repositories and supports the requested 30 issues / 10 clusters / 3 issues per cluster shape:

| Repository cluster | Frozen dataset rows |
| --- | --- |
| `pylint-dev/pylint` | 0, 1, 4 |
| `aws-cloudformation/cfn-lint` | 3, 6, 8 |
| `keras-team/keras` | 5, 23, 45 |
| `falconry/falcon` | 7, 16, 130 |
| `matplotlib/matplotlib` | 9, 11, 57 |
| `sphinx-doc/sphinx` | 10, 14, 34 |
| `instructlab/instructlab` | 12, 35, 231 |
| `python-babel/babel` | 13, 59, 103 |
| `bridgecrewio/checkov` | 19, 28, 72 |
| `patroni/patroni` | 21, 38, 90 |

This is a deliberately narrow, deterministic slice rather than a representative sample of all
SWE-bench-Live. All ten selected repositories are Python ecosystems; the result must not be
described as language- or ecosystem-general without a later, predeclared expansion.

The manifest stores only task identity, the pinned base commit, test metadata, and hashes of the
problem statement, gold patch, and test patch. The model-visible bundle may contain only the
problem statement and a clean checkout at the base commit. The patch, test patch, hints,
`FAIL_TO_PASS`, `PASS_TO_PASS`, difficulty metadata, and reference solution are auditor-only and
must never enter prompts, tool environments, child processes, event logs, or result artifacts.

## Acquisition and audit gates

Run the offline/public-read checks before any provider qualification:

```text
python3 benchmarks/research/swebench-live-v1/acquisition_preflight.py --plan --verify-dataset
```

Acquisition requires the explicit `--acquire --allow-network-write` pair and a new operator cache.
The checkout-only preflight then checks every checkout's exact detached `HEAD` and clean status.
Tracked symlinks are not blanket-rejected: each tracked link must resolve through a relative target
that remains inside its checkout. The acquired 30-root evidence contains 30 such confined links.

```text
python3 benchmarks/research/swebench-live-v1/acquisition_preflight.py \
  --preflight --cache-dir /path/to/operator-cache
```

This script does not call a model, run candidate code, or alter the manifest. The separate
`environment_lock.json` pins the official SWE-bench-Live evaluator source at commit
`a145aa87c62361532dacaa243398978164b234b7` and verifies SHA-256 digests for the evaluator,
Docker/image helpers, and grading assets without storing their solution-bearing data. The full
environment preflight also uses a temporary Docker config and a credential-scrubbed environment;
it never passes provider credentials to Docker or Git:

```text
python3 benchmarks/research/swebench-live-v1/acquisition_preflight.py \
  --environment-preflight \
  --lock benchmarks/research/swebench-live-v1/environment_lock.json \
  --cache-dir /path/to/swebench-live-v1-acquired-345482f7
```

### Contamination-safe authoring inputs

P0 export is a separate read-only operation over the pinned public rows and the acquired clean
base checkouts:

```text
python3 benchmarks/research/swebench-live-v1/authoring_inputs.py \
  --export-p0 \
  --cache-dir /path/to/swebench-live-v1-acquired-345482f7 \
  --output-dir /path/to/operator-p0-bundle
```

The exporter verifies every `problem_statement_sha256`, exact base `HEAD`, and clean status before
writing an allowlisted P0 bundle. The bundle is required to live outside the repository and has no
solution-bearing row fields. P1 is not deterministically authorable from the current evidence:
there is no reviewed task-specific source beyond P0 and the base snapshot. Accordingly the
fail-closed provenance command writes no correction text, assigns no P1 hashes, and exits 2:

```text
python3 benchmarks/research/swebench-live-v1/authoring_inputs.py \
  --author-p1 \
  --cache-dir /path/to/swebench-live-v1-acquired-345482f7 \
  --p0-bundle /path/to/operator-p0-bundle/p0_authoring_inputs.json \
  --output-dir /path/to/operator-p0-bundle
```

The lock records this as `p1_authoring.status=blocked_fail_closed` for all 30 tasks, with
`p1_materialized_count=0` and `p1_hashes_verified=false`. Static and correction cells therefore
cannot be scheduled yet; the shared-P1 hash invariant is retained as a required future check, not
claimed as satisfied.

After Docker recovered, one serial gold smoke was run against the smallest selected task,
`bridgecrewio__checkov-6630`, using its pinned official image and a 2 GiB / one-CPU / no-network
container. The base checkout, both auditor-side applications, and one target test passed. This is
recorded in `benchmarks/research/swebench-live-v1/gold_smoke.json`; it validates one image/task
path only and deliberately leaves the full 30-task hidden oracle `unverified`.

The command returns `preflight_partial` (exit code 2) while Docker, the contamination-safe P0/P1
freeze, or the hidden oracle is unverified. A live run is blocked until all of the following are
separately frozen and audited:

1. reproducible per-repository environments and dependency hashes;
2. candidate construction and hidden behavioral oracles that accept equivalent fixes;
3. proof that the model worktree and prompt builder cannot read auditor-only fields;
4. license/provenance review and a complete terminal-event/credential-scrubbing audit; and
5. a write-once request schedule and provider/model digest.

No missing or failed acquisition may be replaced by a different task after the freeze. If an
oracle or environment is not reproducible, the task remains scheduled and is reported as an
integrity/operational failure rather than silently excluded.

## Provisional schedule and estimands

The confirmatory E2E shape is:

```text
30 tasks × 2 arms (B/C) × 2 request variants (static/correction) × 1 repeat (0)
= 120 model runs per model
```

Arm `B` runs with alignment disabled; arm `C` runs with alignment enabled and automatic evidence
refresh. Each arm is crossed with request variant `static` or `correction`; there is one repeat
label, `0`. Static delivers the complete P1 from call 0. Correction delivers P0 at call 0, then
the same P1 after the first `ExecutionFinished` with exit code 0 and any non-empty
`affected_paths`; this trigger is not filtered to production paths. No candidate-role,
prompt-condition, or three-repeat component-study cells are retained in this E2E manifest. The
primary unit is repository; the three issues and all four cells are nested within that cluster and
are not iid observations. The primary comparison is paired repository-cluster C-minus-B risk
difference in the correction variant, with the static/correction difference-in-differences as a
secondary interaction and correct delivery/coverage as a guardrail.

Each of the 30 scheduled tasks also has a contamination-safe requirement protocol. `P0` is the
original `problem_statement` at the pinned base snapshot. `P1` is a later correction authored only
from `P0` and that base snapshot; it must not use the gold patch, test patch, hints, test lists, or
any other auditor-only field. The static and correction variants use the identical P1 text. The
correction variant injects P1 only after the first qualifying execution event above, and
continuation requires a durable `requirement_revision_ack` event. The manifest deliberately stores
no P1 correction text while this freeze is pending; inventing a correction from the hidden patch
would invalidate the study. The run remains blocked until every task has an auditable P0/P1
authoring record and the acknowledgement path is tested.

Do not pool these eventual observations with the authored fixture or DeepSeek v1/v2 outcomes to
obtain a smaller p-value. Do not stop early for significance, remove a difficult repository, or
replace an invalid call based on its verdict. Every scheduled index remains in ITT accounting;
the confirmatory analysis is suppressed if the predeclared operational-validity/attrition gate is
breached. A future live protocol must set the exact alpha, paired statistic, bootstrap/null
reference, token cap, and stopping conditions before model call 0.

## Current validation

At this stage the public dataset read and deterministic selection checks pass for all 500 source
rows and all 30 selected issues, and the 11 official evaluator asset digests verify against the
locked public commit. Docker is reachable and the selected one-task gold smoke passed. The P0
exporter has produced and re-checked a 30-task operator bundle; P1 authoring correctly fails
closed with zero materialized P1 texts. The manifest remains
`acquisition_pending`; the environment lock remains `preflight_partial` until the Docker probe,
contamination-safe P0/P1 freeze, and hidden oracle are complete. All reports retain zero model
calls and `outcomes: unrun`. The 30 acquired roots are present at their pinned detached
commits and clean; the 30 tracked symlinks resolve inside their respective roots. No candidate
oracle, model response, significance test, or external-validity claim has been produced. The
current canonical manifest digest is
`de5d086a9182a12095bd466b569c73c0b1512459fd26c2c80dae77f6593595d2`.

## Native x86_64 execution path

The final E2E scheduler is `.github/workflows/research-swebench-live.yml`. It runs one task per
job on `ubuntu-24.04`, performs the gold hidden-oracle preflight with the pinned harness, then
runs all four B/C x static/correction cells. The oracle executes the official Docker image and
parser but runs the declared `FAIL_TO_PASS` and `PASS_TO_PASS` node IDs explicitly in bounded
chunks. This avoids unrelated full-suite environment failures while retaining the official test
patch, image, parser, and test lists. Results are sanitized (no hidden test bodies or test lists)
and combined by a final analysis job.
