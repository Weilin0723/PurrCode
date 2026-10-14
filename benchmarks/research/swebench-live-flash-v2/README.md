# ASE oracle-qualified acquisition revision 2

Pre-model acquisition revision; no efficacy observations or model-study freeze exists yet.

The first disjoint candidate set passed official gold qualification for 25/30 instances.
Conan (2/3 failed) and Kedro (3/3 failed) are rejected as whole repository groups before
model generation. Keep their failed qualification evidence. Retain the eight other groups
and select the next two eligible disjoint groups in pinned first-eligible-row order: Faker
and Linkding, with each group's first three eligible issues. No model candidate outcome
is used for eligibility, task replacement, or span authoring. All thirty final tasks now have
passing gold-oracle evidence: 24 from workflow 38514744241 and six from 38522037780.

Reproduce selection with `python3 scripts/ase_disjoint_acquisition.py OUTPUT.json
conan-io/conan kedro-org/kedro`. The original ten repositories and the two oracle-failed
groups are recorded separately in the manifest. Oracle inputs remain quarantined; the
published acquisition contains public issues and opaque hashes, never hidden test bodies.
