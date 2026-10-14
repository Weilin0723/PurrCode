# SWE-bench-Live acquisition manifest

This directory is the real-repository expansion path for the completion study. It is
**acquisition-only**: no model calls, candidate patches, oracle verdicts, or efficacy outcomes
are stored here. The authored multi-repository fixture in `../multi-repo-v1` remains useful for
offline harness checks, but it does not establish external validity and is not pooled with this
suite.

## Frozen source and selection

`manifest.json` pins the `verified` split of
[`SWE-bench-Live/SWE-bench-Live`](https://huggingface.co/datasets/SWE-bench-Live/SWE-bench-Live)
at revision `b51a86422e10cfd403beb4773e5a2947953e36ec`. All 500 verified rows are scanned in
`row_idx` order. A row is eligible when its patch is at most 50 changed lines and its
`FAIL_TO_PASS` and `PASS_TO_PASS` sets are non-empty. Repository groups are ordered by their first
eligible row; the first ten groups with at least three eligible rows are retained, with each
group's first three rows frozen. This yields 30 issues across 10 repositories, three independent
issues per repository. The repository is the inferential cluster; the three issues are nested
within it.

The pinned scan has 370 eligible rows across 87 repositories. The selected repository order is
`pylint-dev/pylint`, `aws-cloudformation/cfn-lint`, `keras-team/keras`, `falconry/falcon`,
`matplotlib/matplotlib`, `sphinx-doc/sphinx`, `instructlab/instructlab`, `python-babel/babel`,
`bridgecrewio/checkov`, and `patroni/patroni`.

Only public metadata and SHA-256 digests of problem statements, patches, and test patches are
frozen. The solution-bearing strings themselves are deliberately absent. Before a live run, an
operator must independently audit candidate construction, hidden-oracle behavior, licenses, and
the execution environment.

## Read-only checks

```text
python3 benchmarks/research/swebench-live-v1/acquisition_preflight.py --plan --verify-dataset
```

`--plan` prints explicit `git` argv vectors without executing them. `--verify-dataset` reads the
pinned public rows endpoint in bounded pages, re-derives the deterministic 30-row selection, and
checks task identity, base commits, test metadata, selection counts, and all three digests. It
performs zero model calls and leaves `outcomes` as `unrun`.

Acquisition is an explicit, separate operation:

```text
python3 benchmarks/research/swebench-live-v1/acquisition_preflight.py \
  --acquire --allow-network-write --cache-dir /path/to/operator-cache
python3 benchmarks/research/swebench-live-v1/acquisition_preflight.py \
  --preflight --cache-dir /path/to/operator-cache
```

The checkout-only preflight requires every checkout to be detached at its pinned base commit and
clean. Tracked symlinks are allowed only when their relative target resolves inside that checkout;
the acquired evidence contains 30 such links and no link is treated as an external dependency. It
does not run tests or expose auditor-only fields to a model.

The contamination-safe P0 exporter is a separate, credential-free operation. It reads the pinned
revision, verifies each problem-statement digest, and records the matching clean base-checkout
identity. Its output directory must be outside this repository so an authoring bundle cannot be
picked up accidentally by a model-visible artifact:

```text
python3 benchmarks/research/swebench-live-v1/authoring_inputs.py \
  --export-p0 \
  --cache-dir /path/to/swebench-live-v1-acquired-345482f7 \
  --output-dir /path/to/operator-p0-bundle
```

The exporter uses an allowlist for every written field; it does not copy solution or oracle
metadata. P1 authoring is currently fail-closed because no reviewed task-specific authoring
source exists that uses only P0 and the base snapshot. The provenance command writes no P1 text,
records a null P1 digest for all 30 tasks, and exits with status 2:

```text
python3 benchmarks/research/swebench-live-v1/authoring_inputs.py \
  --author-p1 \
  --cache-dir /path/to/swebench-live-v1-acquired-345482f7 \
  --p0-bundle /path/to/operator-p0-bundle/p0_authoring_inputs.json \
  --output-dir /path/to/operator-p0-bundle
```

Until a human-reviewed or equivalently auditable P1 source is supplied, the lock keeps
`p1_authoring.status=blocked_fail_closed`, `p1_materialized_count=0`, and the study cannot pass
the environment gate.

The smallest selected task (`bridgecrewio__checkov-6630`) was then exercised serially in its
official `starryzhang` image
(`sha256:bd59eee7174a803d311a588bab4682e826a46774608d1a6587762d45bfd4dcad`) with one CPU, 2 GiB
memory, and no network.
The pinned base checkout, solution application, test application, and one target test all exited
successfully. This is recorded in `gold_smoke.json` and raises only the image/smoke evidence; the
30-task hidden behavioral oracle remains `unverified`.

`environment_lock.json` freezes the official evaluation assets at commit
`a145aa87c62361532dacaa243398978164b234b7` and stores only their SHA-256 digests. The
credential-clean environment preflight verifies those public assets and probes Docker using an
isolated config with no inherited proxy credentials or provider credentials:

```text
python3 benchmarks/research/swebench-live-v1/acquisition_preflight.py \
  --environment-preflight \
  --lock benchmarks/research/swebench-live-v1/environment_lock.json \
  --cache-dir /path/to/swebench-live-v1-acquired-345482f7
```

This command must report `preflight_pass` only after Docker, the contamination-safe P0/P1 freeze,
and the hidden behavioral oracle are verified. Until then it returns `preflight_partial` (exit code 2), keeps model calls at zero, and
leaves outcomes `unrun`. The official harness is an auditor-side evaluator: its patch, test patch,
test lists, and grading fields never enter the model-visible bundle. The E2E arms are `B`
(alignment disabled) and `C` (alignment enabled with automatic evidence refresh), crossed with
request variants `static` and `correction`, once at repeat label `0`: 120 runs per model. Static
delivers complete P1 from call 0. Correction delivers P0 from call 0 and the identical P1 after
the first `ExecutionFinished` with exit code 0 and any non-empty `affected_paths`; no production
path filter is permitted. Candidate-role artifacts from the older component study are not part of
this manifest. Every task is staged as `P0` then `P1`: `P0` is the original problem statement at
the pinned base snapshot, while `P1` is authored only from that P0/base pair and never from gold or
test patches. P1 requires a durable `requirement_revision_ack`; no P1 text is frozen until its
contamination-safe authoring record exists.

Current canonical manifest digest (computed by the preflight):

```text
de5d086a9182a12095bd466b569c73c0b1512459fd26c2c80dae77f6593595d2
```
