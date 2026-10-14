# ASE repository-disjoint oracle qualification candidates

Status: candidate acquisition metadata only. No model-study freeze or efficacy result exists.

The same pinned 500-row verified source and ≤50-changed-line eligibility rule are used as in
`swebench-live-v1`. The ten prior repositories are excluded, then the next ten eligible repository
groups in first-eligible-row order are selected, with each group's first three eligible tasks.
This yields 30 tasks in ten repositories disjoint from the prior live study. Selection does not
consult candidate model outcomes. The earlier model results and oracle failures remain archived.

Reproduce the manifest to a new destination:

```sh
python3 scripts/ase_disjoint_acquisition.py /path/to/new-manifest.json
```

The official oracle runs on x86_64 CI with the immutable dataset revision and pinned harness
commit. Run the research workflow with `oracle_only=true` and this manifest before creating the
model-study freeze. This phase makes zero provider calls. Qualification failure is retained;
changes to candidate selection or environment require a new declared acquisition revision before
model generation. Passing gold tests is necessary for operational qualification, not evidence that
an evaluated agent can solve the task. Hidden test bodies, test lists, and solution strings are
excluded from the model-facing bundle and the exported gold-preflight rows.
