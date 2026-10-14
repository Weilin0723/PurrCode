# Repository-completion v1: preregistration

**Status:** frozen study plan; no model result is represented here  
**Freeze date:** 2026-10-12  
**Scope:** small-model completion-verdict study, not a coding-agent efficacy claim

**Pre-call clarification (2026-10-12):** the planned enumeration of all `2^6` sign flips is a
descriptive null-reference interval, not a confidence interval for the observed mean. This
terminology correction was made after executable-protocol review and before any main-study model
call; it changes neither the observations, estimand, test, alpha, nor stopping rule.

**Pre-call integrity clarification (2026-10-12):** the immutable pre-call contract consists of a
write-once study freeze binding the complete 288-call schedule and both model digests, plus a
write-once run freeze for each 144-call model block. Both bind this plan, the six-candidate
dataset, dependencies, decoding parameters, and schedules. Request/response events are
append-only. The human-readable manifest is a versioned run-state snapshot and may be atomically
rewritten as the run advances; it is not itself described as append-only. This clarification was
made before any main-study call and does not change an outcome or analysis rule.

This document is the analysis contract for the next small-model study. The task list, candidate
types, prompt conditions, model roles, seeds, scoring rules, primary contrast, exclusion rules,
and stopping rules are fixed before the first main-study model call. A result is not changed by
adding a task, replacing a failed call, choosing a more favourable seed, or editing a candidate
after seeing a verdict.

## 1. Research questions and hypotheses

**RQ1 (primary).** Does conjoining a model's completion verdict with the deterministic
revision/artifact gate reduce externally false-ready verdicts relative to the same model verdict
alone?

**RQ2 (guardrail).** Does that reduction preserve useful completion behavior, measured by correct
delivery and claim coverage on externally correct artifacts?

**RQ3 (secondary).** How do the result and the C-versus-B contrast vary with the plain versus
revision-rubric prompt, with the 1B secondary model, and with gate-only scoring?

The primary hypothesis is that C has a lower external false-ready rate than B. The null is zero
median paired cluster difference; the alternative is a non-zero difference, tested two-sided so
that an unexpected increase is visible. The primary family contains exactly one contrast:
`llama3.2:3b` under `revision_rubric_json`, C versus B. No significance claim is made for a
smoke check, the plain prompt, the 1B model, or D.

The usefulness guardrail is a predeclared 5 percentage-point non-inferiority margin. A lower
false-ready rate is not interpreted as a desirable result if C's correct-delivery rate or claim
coverage is more than 5 percentage points below B's corresponding rate. With six independent
base-task clusters this is a guardrail for interpretation, not a claim that the pilot can
establish non-inferiority.

## 2. Frozen data and independent labels

The study has six base tasks from the frozen small-model-pilot fixture:

1. `pagination-tail`
2. `century-leap-year`
3. `scheduler-reporter-ids`
4. `http-client-retry-parity`
5. `settings-label-preservation`
6. `retry-exponential-delay`

Each base task has six candidate artifacts, always in this order:

| Candidate | Artifact construction | Hidden external label | Fixed revision/artifact metadata |
| --- | --- | --- | --- |
| `baseline` | Unmodified fixture | incorrect | stale binding |
| `gold` | Minimal intended repair | correct | current |
| `stale` | Repair plus the predeclared stale/incorrect change | incorrect | stale binding |
| `overbroad` | Gold repair plus an unrelated behavior change | incorrect | current |
| `test_tampering` | Gold repair plus removal/weakening of a test or integrity check | incorrect | current |
| `gold_stale_evidence` | Gold repair with otherwise correct behavior but stale evidence binding | correct | stale binding |

There are therefore 36 unique candidate artifacts. The candidate names and the external labels
are harness metadata and are never included in a model prompt. The exact operations, source-file
hashes, oracle paths, and expected labels are taken from the frozen dataset manifest. A pre-model
preflight must verify all 36 candidate constructions and all 36 labels; a mismatch is a study
integrity failure, not a reason to repair the candidate and continue.

The currently existing five-candidate design-only manifest is only a source for the first five
constructions. The sixth `gold_stale_evidence` construction, its hashes, and its oracle result
must be added to a new frozen manifest before any model call; rows from the five-candidate
preflight cannot be silently relabelled or reused as the six-candidate study.

The label is produced by a hidden behavior oracle outside the agent worktree. It compiles or runs
behavioral checks against a fresh candidate copy and also checks the predeclared preservation and
test-integrity requirements. The oracle is run only after the candidate snapshot is immutable;
its source, harness, label, and output are not visible to the model, agent, gate input, or prompt
builder. It must accept behaviorally equivalent implementations rather than compare only with the
gold diff. The oracle is an evaluator, never an authorization path and never a source of evidence
fed back to the model.

The deterministic gate receives only the frozen revision/artifact metadata and the ordinary
model-visible evidence. The mapping above is fixed before calls: `gold` is current;
`baseline`, `stale`, and `gold_stale_evidence` use stale binding; and
`overbroad`/`test_tampering` use current binding. The gate does not receive the hidden oracle
label. This tests both the coverage cost of stale evidence on a behaviorally correct artifact
(`gold_stale_evidence`) and whether current metadata can still be insufficient for semantic
correctness in the overbroad and test-tampering cases.

## 3. Conditions, models, and prompts

For a candidate, let `M` be the parsed model verdict and `G` be the deterministic gate verdict:

| Score | Definition | Role |
| --- | --- | --- |
| B | `M` | model verdict alone; primary baseline |
| C | `M AND G` | model verdict conjoined with the gate; primary intervention |
| D | `G` | gate-only diagnostic; no model call |

The primary model is exactly `llama3.2:3b`, with its provider/model digest recorded before the
first call. `llama3.2:1b` is secondary and exploratory. A model identifier without a resolved
digest is not sufficient to start the study.

There are two frozen JSON-output prompt conditions:

* `revision_rubric_json` is the only primary prompt condition. It asks the reviewer to check the
  current requirement revision, artifact digest, each required observation, and preservation
  constraints, then return exactly `{"verdict":"READY"|"BLOCKED","reasons":[string...]}`.
* `plain_json` asks for the same binary completion verdict and JSON shape using the request,
  artifact, and evidence, but does not provide the revision/artifact checklist. It is a
  predeclared secondary prompt sensitivity and does not contribute to the primary p-value.

Both prompts are frozen byte-for-byte, use the same system/data boundary, and omit candidate IDs,
hidden labels, and oracle outputs. The model sees only the request/correction text, the artifact
snapshot or diff selected by the harness, and the allowed evidence/metadata. Temperature, top-p,
maximum output tokens, stop sequence, provider endpoint, and response parser are fixed in the
manifest before calls. The planned decoding is temperature 0, top-p 1, bounded JSON output, with
no response repair or outcome-dependent retry. Seeds 17 and 29 are recorded for every call.

An exact valid `READY` is the only positive model verdict. `BLOCKED`, invalid JSON, schema
failure, timeout, and provider failure are operationally non-ready for B and C, while their
failure status remains separately recorded. This conservative convention prevents malformed
output from becoming a completion claim; claim coverage and failure attrition prevent an
always-invalid system from being described as useful.

## 4. Schedule and feasibility gate

The complete model-call schedule is:

| Model | Prompt | Seeds | Tasks × candidates | Scheduled calls | Analysis role |
| --- | --- | --- | ---: | ---: | --- |
| `llama3.2:3b` | `revision_rubric_json` | 17, 29 | 6 × 6 | 72 | primary B/C |
| `llama3.2:3b` | `plain_json` | 17, 29 | 6 × 6 | 72 | secondary sensitivity |
| `llama3.2:1b` | `revision_rubric_json` | 17, 29 | 6 × 6 | 72 | secondary B/C |
| `llama3.2:1b` | `plain_json` | 17, 29 | 6 × 6 | 72 | secondary sensitivity |
| **Total model calls** |  |  |  | **288** |  |

D is evaluated once for each of the 36 unique candidates. For descriptive aligned tables its
fixed verdict may be copied to the corresponding prompt/seed cells, but those copies are not
independent observations and are never used to inflate an inferential denominator.

The independent unit is the **base-task cluster**. The six candidates share a task, fixture, and
oracle; seeds 17 and 29 repeat the same candidates and are explicitly **not independent**. The
primary false-ready cluster contains the 8 external-blocked observations (four candidates × two
seeds); its guardrail additionally contains the 4 externally correct observations from `gold` and
`gold_stale_evidence`. No candidate-level binomial test or seed-level replication claim is
allowed.

Before the main schedule, run exactly three fixed feasibility smoke cases using fresh worktrees:
one known `gold`, one known `gold_stale_evidence`, and one known `test_tampering` candidate. Each smoke case
must prove worktree/oracle isolation, reproduce its frozen external label, execute the gate, and
persist a complete trace without leaking credentials or hidden labels. Smoke outputs are
feasibility evidence only. The smoke gate is go/no-go: if a required smoke attempt fails, the
main schedule is not started; in particular, three failed smoke attempts are never pooled into
the main result. A smoke failure is not replaced with a more convenient case or counted as a
model outcome.

The study schedule is frozen in four blocks: primary 3B revision-rubric (72 calls), primary 3B
plain (72), secondary 1B revision-rubric (72), then secondary 1B plain (72). Within a block the
order is task, candidate, then seed. Each call reviews an immutable candidate snapshot whose
hash and prompt hash were frozen before the block began. All 288 scheduled calls are run unless
a predeclared safety/integrity stop occurs. There is no optional stopping, interim significance
check, task deletion, seed substitution, best-of-N selection, or post-result candidate edit.

## 5. Outcomes and estimands

Let `E=1` mean that the hidden oracle accepts the candidate as externally correct, `M=READY`
mean that the model makes a completion claim, and `G=READY` mean that the deterministic gate
allows delivery. For each score `A` in `{B,C,D}`, define:

* **External false-ready:** on the external-blocked stratum (`E=0`), the rate of
  `1[A=READY]`. This is the primary outcome. Its denominator contains only the four externally
  rejected candidate types (`baseline`, `stale`, `overbroad`, and `test_tampering`); all scheduled
  rows remain in ITT accounting.
* **Correct delivery:** `K_A = 1[A=READY AND E=1]`. This guards against the trivial strategy of
  blocking everything. The guardrail includes both externally accepted candidate types:
  `gold` and `gold_stale_evidence`; report them separately as well as pooled.
* **Claim coverage:** among externally correct candidates, the proportion with `A=READY`. For B
  and C this is model-visible claim coverage over `gold` plus `gold_stale_evidence`; for D it is
  reported as gate readiness, not as an actual model claim. The stale-evidence case is retained
  specifically so this guardrail exposes a safety/coverage trade-off rather than rewarding an
  always-blocking gate.
* **Gate-only diagnostics:** D's readiness, B-to-C filtering count, current-versus-stale metadata
  behavior, invalid-output rate, latency, and token usage.

For B and C, the primary estimand is the paired cluster rate difference
`d_t = mean(F_C,t | E=0) - mean(F_B,t | E=0)` over the 8 external-blocked observations in base
task `t`. Lower is better. Report all six `F_B,t`, `F_C,t`, and `d_t`, the paired mean and median,
and the number of claims filtered by G. Report the 4-observation correct-delivery/coverage
stratum separately. Report D separately because it has no model claim and only 36 unique gate
evaluations.

Also retain external labels and scores by candidate type, prompt, seed, and model, but treat
those slices as descriptive. Record lifecycle/gate state, revision binding, artifact digest,
model claim, oracle result, correction/evidence status, provider status, token usage, latency,
and final trace hash for every scheduled cell.

## 6. Primary and secondary statistics

The primary test is a two-sided exact paired sign test on the six cluster differences `d_t`, with
alpha 0.05. A nonzero `d_t` is a C win when it is negative, a C loss when it is positive, and a
tie when it is zero; ties are excluded from the sign count but remain reported. The p-value is
the exact two-sided sign-test probability under equal win/loss probability. No normal, iid
candidate, or iid seed approximation is used. With six clusters, even six directional wins are
the smallest attainable two-sided result of 0.03125; this is a property of the design, not a
promise that the result will be significant.

The effect estimate is the paired mean cluster difference with the six cluster values shown in
full. A 95% exact sign-flip null-reference interval (all `2^6` sign assignments, with ties fixed
at zero) is reported descriptively and is not labelled a confidence interval. If an
implementation cannot produce that interval from the frozen manifest, report the six differences
without substituting an asymptotic interval.

Secondary analyses are:

1. The same C-versus-B cluster summary for `llama3.2:1b` under
   `revision_rubric_json`.
2. Plain-prompt sensitivity for both models, never pooled into the primary p-value.
3. D versus B and D versus C false-ready, correct-delivery, and claim/readiness coverage,
   reported as diagnostics rather than a new confirmatory family.
4. Candidate-stratified results, especially current-but-externally-wrong overbroad and
   test-tampering candidates versus stale-binding baseline/stale candidates, with
   `gold_stale_evidence` reported as the externally-correct stale-binding stratum.

Secondary p-values, if displayed, are labelled exploratory and Holm-adjusted within the
predeclared secondary family. They cannot overturn the primary decision. No result is described
as a method win unless the primary direction is accompanied by the correct-delivery and claim
coverage guardrails.

## 7. ITT, failures, exclusions, and stopping

The primary intention-to-treat denominator is the 72 scheduled primary model calls. Every
scheduled call remains in the manifest and final report, including provider errors, timeouts,
invalid JSON, empty output, and interrupted traces. Such model-side failures are non-ready for B
and C and are separately counted as attrition; they are not silently removed or replaced.

If the external oracle returns an explicit rejection, `E=0`. If it returns an explicit acceptance,
`E=1`. If it fails or times out after the main schedule has started, `E=unknown`; the call remains
in ITT. For the prespecified conservative primary bound, a `READY` verdict with `E=unknown` is
counted as false-ready and a non-ready verdict is not; also report the unknown count and an
available-label sensitivity table. Unknowns are never presented as external correctness.

Only the following pre-call conditions can stop the study without a main result: fixture/oracle
hash mismatch; inability to keep the hidden oracle outside the agent worktree; credential or
label leakage; missing model digest; or failure of the three-case smoke gate. These are
feasibility/integrity failures, not outcome-based exclusions. After main calls begin, no task,
candidate, prompt, seed, or model is removed because its result is inconvenient. A storage or
cleanup failure is retained as an ITT failure and marked uncertain; it does not trigger a
replacement run.

The schedule runs to completion. A provider restart, process retry, or host interruption may be
recorded for diagnosis, but it does not authorize a fresh replacement observation. No optional
stopping is permitted after a favourable p-value, and no additional task or seed may be added to
increase power.

## 8. Reproducibility record

Before the first main call, persist a write-once freeze record containing: this plan's hash;
dataset, fixture, candidate-operation, oracle, prompt-template, parser, and gate-script hashes;
resolved model digest; provider endpoint and local/remote mode; decoding settings; and the complete
scheduled task/candidate/prompt/seed indices. Append-only events and versioned run-state snapshots
then retain task/candidate metadata; source and final artifact hashes; complete model response;
parsed verdict; gate verdict; hidden-oracle verdict; token usage; latency; timeout/error status;
and environment sanitization status. These records must not contain provider credentials or expose
hidden labels to the model process. Store smoke artifacts separately from main-study artifacts and
never merge their rows into the efficacy table.

The oracle runs after the worker/call has stopped and the worktree has been copied or snapshotted.
Validation has a bounded timeout and a sanitized environment. A run whose worktree is still
changing is uncertain until cleanup is confirmed; scoring a moving tree is prohibited.

## 9. Validity boundary and interpretation

This design addresses one narrow construct: whether a small model's binary completion verdict is
filtered by revision/artifact metadata in a six-task synthetic repository fixture. The hidden
behavior oracle improves construct validity over deterministic labels derived from the same gate,
but the study is still an authored component study. It does not establish general coding-agent
success, repository-level productivity, human-perceived usefulness, or external validity across
projects.

The six tasks come from one fixture family, and both seeds repeat the same candidates. The
revision-rubric prompt can make the gate's vocabulary salient; that is why plain JSON is retained
as a secondary sensitivity, not pooled into the primary result. Current metadata on
overbroad/test-tampering candidates tests the limitation of metadata-only gating but does not
prove that the gate is a semantic oracle. Six clusters provide little generalization precision;
all cluster values and attrition are therefore reported.

The study must not be described as submission-ready, as proof of coding-agent efficacy, or as
significant before the frozen primary analysis is run. A significant primary p-value, if one is
obtained, supports only this preregistered component contrast and must be accompanied by the
guardrails, failure accounting, and validity limitations above.
