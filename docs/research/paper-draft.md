# Freshness Is Not Correctness: Auditing Revision-Bound Completion in Coding Agents

**ASE research-track working manuscript — component evidence; E2E qualification open**
**Date:** 2026-10-14
**Artifact status:** complete 288-call local study plus audited 144-call DeepSeek operational repair  

## Abstract

When a user changes a software requirement, a coding agent can reuse validation or review evidence
that refers to an obsolete request or artifact. We investigate whether revision- and artifact-bound
completion checks distinguish trustworthy delivery from merely current evidence. A deterministic
binding gate filters a model's completion verdict; it does not evaluate semantic correctness.
Because conjunction can only remove deliveries, our central question is which decisions it removes
and what availability it sacrifices.

A preregistered component study constructed 36 executable snapshots from six authored Rust tasks,
including current-but-incorrect and correct-but-stale counterexamples. Independent behavioral,
preservation, and test-integrity oracles supplied external labels. Two local models, two prompt
conditions, and two replicate labels yielded 288 scheduled calls. In the primary 3B condition,
filtering reduced false-ready decisions from 23/48 to 13/48, while correct coverage fell from 10/24
to 5/24 and violated the predeclared five-point loss margin. A separately frozen, outcome-aware
DeepSeek output-cap repair produced 143/144 valid responses: false-ready fell from 30/48 to 12/48,
but correct coverage fell from 24/24 to 12/24. Current binding admitted over-broad and test-tampering
repairs; stale binding rejected behaviorally correct code.

These authored counterexamples establish a mechanism boundary and an availability cost, rather
than an estimate of real-world failure prevalence or a general improvement in coding agents.
They motivate measuring evidence refresh separately from static evidence filtering. The
cross-repository live study is still unqualified and contributes no efficacy claim to this draft.

## 1. Introduction

Software tasks rarely remain a single immutable prompt. A user may correct a requirement after an
agent has planned, edited files, or run validation. A plausible but unsafe agent can continue from
the old interpretation, reuse stale test evidence, and report completion. Compilation and visible
tests do not prove that the final request was satisfied, particularly when the change concerns
preservation constraints or behavior outside the visible suite.

This paper treats completion as a revision-sensitive decision rather than a free-form model
assertion. Evidence records are bound to a requirement revision and artifact digest. A correction
invalidates affected evidence, and a delivery decision becomes ready only when evidence matches
the current request and artifact. A hidden research oracle remains separate: it evaluates the
immutable candidate after construction but never authorizes execution or supplies facts to the
model.

The distinction matters because freshness and correctness are different constructs. A stale
evidence record can accompany correct code, and a current evidence record can accompany an
over-broad or test-tampering patch. A revision gate may therefore reduce unsafe delivery while also
rejecting useful work. Measuring false-ready decisions without measuring correct-delivery coverage
would reward a system that blocks everything.

We implement and evaluate this mechanism in an anonymized governed coding-agent prototype with typed
actions, durable authorization, execution-time digest checks, isolated worktrees, requirement
contracts, fresh review, bounded correction, and a delivery reducer. The present paper does not
claim end-to-end coding-agent efficacy. An attempted 3B end-to-end feasibility run could not
complete within the local runtime budget, so the confirmatory small-model evidence is deliberately
scoped to completion decisions over executable repository snapshots.

We ask three research questions:

- **RQ1 (safety):** How much does binding reduce false-ready decisions on externally rejected candidates?
- **RQ2 (availability):** How much externally correct work does the same filter block?
- **RQ3 (failure boundary):** Which current-but-wrong and correct-but-stale cases distinguish freshness from correctness?

The implemented live extension asks whether evidence refresh changes this trade-off during actual coding. That question remains unanswered by the completed component studies.

This work contributes:

1. a revision- and artifact-bound completion rule that is explicitly separated from a semantic
   oracle;
2. a frozen six-task, six-candidate evaluation that includes both current-but-wrong and
   correct-but-stale counterexamples;
3. a preregistered 288-call small-model study with task-clustered inference, intention-to-treat
   failure handling, write-once freeze records, and independent recomputation; and
4. evidence of a measured false-ready reduction together with a failed coverage
   guardrail, exposing rather than hiding the mechanism's safety–availability trade-off;
5. an audited DeepSeek V4 Pro operational-repair replication that preserves the same trade-off
   while explicitly retaining the failed first remote run.

## 2. System and intervention

### 2.1 Governed execution baseline

The prototype represents tool proposals as typed actions rather than shell strings. Its policy
evaluator applies deterministic rules; a durable session store persists decisions and exact
single-use authorization; and the execution adapter rechecks the serialized action and constraints
before spawning an explicit argument vector.
Credentials are excluded from model context and tool environments. Session leases, isolated
worktrees, checkpoints, and durable events support recovery.

The alignment path stores quoted user requirements, evidence citations, review findings,
corrections, and delivery assessments. Model-visible validation and review can inform a delivery
decision, but hidden research tests cannot. This separation prevents an evaluator from silently
becoming an execution or authorization path.

### 2.2 Revision and artifact binding

Let \(R_t\) be the active requirement revision, \(A_t\) the reviewed artifact digest, and \(E_t\)
the evidence record. The deterministic gate returns ready only when:

\[
E_t.revision = R_t,\quad E_t.artifact = A_t,\quad A_t.revision = R_t.
\]

The full prototype additionally checks requirement-scoped observations, preservation facts,
review freshness, and authorization. The component study isolates the three binding predicates so
their effect can be distinguished from semantic checking. The gate never reads the hidden
behavior oracle.

For a model verdict \(M\) and deterministic gate verdict \(G\), the study defines:

- **B:** \(M\), the model completion verdict alone;
- **C:** \(M \land G\), the same verdict conjoined with binding checks; and
- **D:** \(G\), a gate-only diagnostic evaluated once per unique candidate.

B and C use the same model response. Their difference therefore estimates filtering by the gate,
not an extra model call or token budget.

### 2.3 What conjunction can and cannot establish

For a fixed model response, C can only remove deliveries allowed by B. For any external label Y,
`Pr(C=READY, Y=incorrect) ≤ Pr(B=READY, Y=incorrect)`. The same set inclusion applies to correct
candidates. Consequently, a directional safety improvement is structurally expected from a
nontrivial filter; a small p-value alone cannot establish that binding is a useful intervention.
Our empirical questions concern its magnitude, where removed decisions occur, and the availability
cost. The frozen guardrail is essential to distinguish useful selectivity from indiscriminate blocking.

| External behavior | Evidence binding | Required interpretation |
| --- | --- | --- |
| Correct | Current | Eligible for delivery, subject to model verdict |
| Incorrect | Current | Freshness cannot establish correctness |
| Correct | Stale | Revalidation is needed; code need not be changed |
| Incorrect | Stale | Blocking prevents delivery, without diagnosing the semantic defect |

The first three binding predicates are the isolated component intervention. The production
review-and-delivery loop performs additional reviews and can refresh evidence; it is a different
intervention. Its generated patches, extra model calls, latency, and validation costs must be
measured separately rather than inferred from the component contrast.

## 3. Preregistered study

### 3.1 Tasks, candidates, and external labels

The frozen dataset contains six base tasks from a Rust repository fixture:

- pagination tail preservation;
- Gregorian century leap years;
- scheduler/reporter identifier consistency;
- HTTP-client retry-policy parity;
- settings-label and option preservation; and
- exponential retry delay.

Each task has six candidate snapshots. `baseline` is unchanged and wrong; `gold` is a minimal
correct repair; `stale` includes a predeclared incorrect change and stale binding; `overbroad`
adds an unrelated behavior change to the repair; `test_tampering` weakens an existing test; and
`gold_stale_evidence` has the same correct code as gold but stale evidence binding. Thus each task
has four externally rejected and two externally accepted candidates.

External labels combine three checks: a Rust behavioral harness located outside the copied
worktree, preservation checks on declared paths, and test-integrity checks. Oracle infrastructure
failures are `UNKNOWN`, not semantic rejection; the main preflight contained zero unknowns. All 36
candidate labels matched the frozen expectations before model calls. Candidate names, expected
labels, and oracle output were absent from prompts.

### 3.2 Models, prompts, and schedule

The primary model was local `llama3.2:3b` with digest
`a80c4f17acd55265feec403c7aef86be0c25983ab279d83f3bcd3abbcb5b8b72`. The secondary model was
`llama3.2:1b` with digest
`baf6a787fdffd633537aa2eb51cfd54cb93ff08e28040095462bb63daf552878`, served by Ollama 0.4.2.
The primary `revision_rubric_json` prompt made request/artifact/evidence binding explicit. The
secondary `plain_json` prompt requested the same strict verdict without the binding checklist.
Both received identical task requirements, preservation constraints, unified diff, changed paths,
public fixture-test status, revision metadata, and digests.

Responses had to be exactly `{verdict, reasons}`, with a `READY` or `BLOCKED` verdict and an array
of reason strings. Temperature was 0, top-p 1, context 4,096, output limit 32 tokens, and seeds 17
and 29. Invalid JSON, timeout, provider failure, or empty output counted as non-ready for B and C
and remained in the intention-to-treat denominator.

The write-once study freeze fixed four 72-call blocks before generation: 3B/rubric, 3B/plain,
1B/rubric, and 1B/plain. The total was 288 calls. Repeated seeds were not treated as independent
tasks.

### 3.3 Outcomes and inference

The primary outcome was external false-ready rate among the four externally rejected candidates
per task. Correct-delivery coverage among `gold` and `gold_stale_evidence` was the principal
guardrail. The predeclared usefulness margin allowed at most a five-percentage-point loss in
coverage or correct delivery.

For each base task, we computed C-minus-B false-ready risk over eight observations: four rejected
candidates and two seeds. The sole confirmatory test was a two-sided exact paired sign test over
the six task-cluster differences at alpha 0.05. We report the paired mean risk difference, the six
cluster values, a task-cluster bootstrap interval, and a descriptive exact sign-flip null-reference
interval. The plain prompt and both 1B analyses were secondary; their three exact sign-test values
were Holm-adjusted as one exploratory family.

No outcome-dependent retry, task deletion, seed replacement, or optional stopping was permitted.
A write-once study freeze bound the plan, dataset, dependencies, both model digests, decoding
parameters, candidate hashes, and all 288 schedule cells. Each model run additionally stored an
immutable run freeze, append-only events, results, summary, and audit.

### 3.4 DeepSeek model-external follow-up

After completing the local study, we froze a 144-call follow-up on the official
`deepseek-v4-pro` API using the same 36 snapshots, prompts, schedule order, external labels,
estimand, guardrail, and ITT handling. The generic provider qualification suite passed 7/7 cases
before the study. The API alias is dated service state rather than an immutable weight digest, and
the replicate labels are schedule metadata rather than claimed API seeds.

The v1 remote freeze used a 128-token output cap. Although all calls terminated, 26 responses hit
the cap and ended as incomplete JSON, exceeding the predeclared 10% attrition threshold. We kept
that run immutable and operationally inconclusive. A separate outcome-aware v2 plan repaired only
the cap (256 tokens), reran all 144 cells, and did not reuse or pool v1 responses. This repair was
motivated by the ceiling-linked failures, not the observed effect. Consequently, v2 is a
transparent operational follow-up rather than an independent confirmation.

### 3.5 AI assistance in the research workflow

The Llama 3.2 models described above are the evaluated systems. Separately, AI research assistants
helped design adversarial candidates, implement and review the harness, run local experiments, and
edit the manuscript. Their outputs did not determine external labels or reported observations.
Executable Rust oracles and deterministic preservation/integrity checks produced labels, while
statistics were recomputed from the frozen raw evidence. Protocol and code changes were inspected
before the study freeze; missing runs, observations, and citations were not generated by an AI.

## 4. Results

### 4.1 Completeness and output attrition

Both model runs completed all 144 scheduled cells and passed the executable audit; the combined
study therefore contains 288/288 scheduled observations. The 3B model produced 64/144 strict valid
responses; all 80 invalid cells were 30-second transport timeouts. The 1B model produced 93/144
strict valid responses; all 51 invalid cells were truncated or otherwise unterminated JSON. Every
strictly valid response from both models declared `READY`. Failures were retained as non-ready,
making output reliability part of the measured system rather than an exclusion.

### 4.2 Primary result: false-ready reduction

Under the preregistered 3B revision-rubric condition, B made 23 false-ready decisions among 48
externally rejected observations (47.9%). Conjoining the gate reduced this to 13/48 (27.1%). The
paired mean difference was −20.8 percentage points; the task-cluster bootstrap 95% interval was
[−29.2, −14.6] points. All six task clusters favored C:

| Base task | C − B false-ready risk |
| --- | ---: |
| Century leap year | −12.5 pp |
| HTTP retry parity | −25.0 pp |
| Pagination tail | −12.5 pp |
| Retry delay | −25.0 pp |
| Scheduler/reporter IDs | −12.5 pp |
| Settings preservation | −37.5 pp |

The exact sign test produced 6 wins, 0 losses, and 0 ties, with two-sided `p=0.03125`. We retain this preregistered numerical output, but conjunction makes
positive C-minus-B differences impossible. The sign-symmetry assumption does not establish useful
selectivity in this paired filtering design; this p-value is not evidence of a general method win. Enumeration of all
64 paired-cluster sign assignments gave the preregistered descriptive 95% null-reference interval
[−16.7, +16.7] points. This interval describes the sign-flip null distribution; it is not a
confidence interval for the observed effect.

### 4.3 Guardrail failure

The false-ready reduction was not free. On the 24 externally correct observations, B delivered
10 (41.7% coverage), whereas C delivered 5 (20.8%). The −20.8-point coverage change exceeds the
predeclared five-point loss margin. Correct delivery as a proportion of all 72 scheduled primary
cells likewise fell from 10/72 (13.9%) to 5/72 (6.9%). The study therefore does not support an
unqualified method-win claim.

The candidate strata explain the trade-off. C removed all five B claims on stale `baseline` and
all five on `stale`, but it did not remove seven over-broad or six test-tampering claims because
those snapshots carried current binding metadata. Conversely, C removed all five claims on
`gold_stale_evidence`, whose behavior was correct but evidence binding was stale. The gate detects
freshness, not semantics.

### 4.4 Gate-only and secondary diagnostics

Across 36 unique candidate snapshots, D was ready for 18. It accepted 6/12 externally correct
candidates and 12/24 externally incorrect candidates. The incorrect ready cases were the
current-bound over-broad and test-tampering snapshots; the correct blocked cases were the
stale-evidence gold snapshots. This symmetric 50%/50% diagnostic confirms that binding alone is
not a correctness oracle.

The 3B plain-prompt sensitivity had the same aggregate −20.8-point risk difference but only four
winning clusters and two ties (`p=0.125`). For 1B, the revision-rubric difference was −41.7 points
with six directional wins; the plain-prompt difference was −37.5 points with five wins and one
tie. These were secondary by design. The raw secondary sign-test values were 0.03125, 0.0625, and
0.125; Holm-adjusted values were 0.09375, 0.125, and 0.125, so none met 0.05. The 1B rubric also
lost coverage from 21/24 (87.5%) to 10/24 (41.7%).

### 4.5 DeepSeek operational-repair replication

The initial DeepSeek run produced 118/144 valid responses and 26 ceiling-linked JSON errors
(18.1% attrition), so its inferential outputs were suppressed under the frozen rule. The separate
v2 repair produced 143/144 valid responses (0.69% attrition), used 106,691 provider-reported
tokens, completed every schedule cell, and passed independent artifact recomputation.

In the repaired primary block, B made 30/48 false-ready decisions (62.5%) and C made 12/48
(25.0%), a −37.5-point paired difference. Task differences were −25, −25, −50, −50, −50, and −25
points; the two-sided exact sign test was `p=0.03125`, and the task-cluster bootstrap 95% interval
was [−45.8, −29.2] points. However, correct coverage fell from 24/24 to 12/24 (−50 points), because
the gate retained all current-bound gold claims but blocked every correct stale-evidence claim.
The unchanged five-point guardrail therefore failed. The plain-prompt secondary block showed a
similar −39.6-point descriptive difference.

### 4.6 Evidence ledger and live-study audit

| Study | Scheduled calls | Valid responses | Primary false-ready B → C | Correct coverage B → C | Interpretation |
| --- | ---: | ---: | --- | --- | --- |
| Local 3B | 144 | 64 | 23/48 → 13/48 | 10/24 → 5/24 | Confirmatory component contrast; guardrail failed |
| Local 1B | 144 | 93 | Secondary analysis | Secondary analysis | Exploratory; no adjusted significant finding |
| DeepSeek v1 | 144 | 118 | Inference suppressed | Inference suppressed | Operationally inconclusive |
| DeepSeek v2 | 144 | 143 | 30/48 → 12/48 | 24/24 → 12/24 | Outcome-aware repair; guardrail failed |
| SWE-bench-Live | 120 sessions planned | Not qualified | Not reported | Not reported | Cross-repository E2E gate open |

An operator audit on 2026-10-14 recovered 108 cells from 27 task artifacts in the completed
x86_64 workflow. Of these, 104 recorded failed sessions and four omitted session status;
106 had unknown oracle outcomes, 102 had empty patches, and only one patch was externally
accepted. The twelve unrecovered scheduled cells remain missing. These observations do not
qualify the intended comparison or identify one common provider failure cause. They establish
that workflow success is insufficient evidence of experimental validity. A subsequent runner repair classifies
failed/cancelled sessions as errors and unknown correctness as missing; artifact aggregation also
preserves task directories to avoid filename collisions. Existing frozen records remain unchanged.
Any repaired study must use a new runner lock and pre-call freeze.

Before new model generation, a repository-disjoint acquisition qualified the official reference
patches. The first thirty candidates included five gold-oracle failures in two repository groups;
both whole groups were rejected, their failures retained, and the next two eligible groups selected
in the pinned acquisition order. Twenty-four retained tasks and six replacements yielded thirty
passing reference-patch qualifications across ten repositories. This is oracle-environment evidence,
not a measurement of agent efficacy. Synthetic correction preflights and the earlier failed live
study are excluded from the prospective efficacy analysis.

## 5. Discussion

The result supports revision binding as a targeted stale-evidence filter. It does not support
treating the gate as proof of task correctness. A production system needs both dimensions:
freshness checks for evidence lineage and semantic validation for behavior and preservation.
Neither can substitute for the other.

The failed guardrail is substantively useful. A benchmark reporting only false-ready rate would
call C a success, while an always-blocking policy would score even better. Including a
behaviorally correct stale-evidence candidate makes the availability cost observable. Systems
should either refresh evidence automatically after confirming an unchanged correct artifact or
surface a precise blocked state that invites revalidation rather than silently discarding useful
work.

The all-READY pattern among valid model responses also cautions against interpreting the study as
model competence. At these sizes and output constraints, both local models were poor binary gates.
The confirmatory contrast concerns deterministic filtering of their decisions, not coding skill or
human-equivalent review.

The DeepSeek follow-up changes that interpretation only modestly. Its high format validity removes
the local models' dominant transport confound and reproduces the task-consistent filtering effect,
but the gate's fixed 50% accepted-snapshot coverage shows that a stronger reviewer cannot repair
the binding policy's availability cost. Automatic evidence refresh is the appropriate next
intervention; relaxing the guardrail after observing the result is not.

## 6. Threats to validity

**Construct validity.** Candidates are repository snapshots, not patches generated end to end by
the evaluated models. The outcome is a completion verdict, not developer productivity or issue
resolution. Hidden behavior, preservation, and integrity checks improve external labeling but do
not cover every semantic property.

**Statistical conclusion validity.** Six task clusters provide the minimum possible two-sided
sign-test value of 0.03125 when all clusters agree. The bootstrap interval is unstable with six
clusters, and seeds repeat the same candidates. The p-value establishes direction within this
fixture, not population-wide precision.

**Attrition and model validity.** Only 44.4% of 3B calls and 64.6% of 1B calls produced strict valid
JSON. Intention-to-treat handling prevents favorable deletion, but timeout and truncation behavior
is part of the observed effect. Both local models failed provider qualification for coding/high-risk
roles. DeepSeek passed the generic suite, but its first research run exceeded the frozen attrition
limit; the repaired run is outcome-aware and uses a mutable dated API alias rather than pinned
weights.

**External validity.** The six tasks come from one authored Rust fixture. Candidate mutations are
systematic counterexamples, not a random sample of real repository failures. Results should not be
generalized to other repositories, languages, providers, or human workflows without replication.

**Researcher degrees of freedom.** The candidate set, models, prompts, seeds, primary contrast,
failure handling, guardrail, and stopping rule were frozen before calls. Two independent pre-call
reviews blocked execution until dataset, oracle, schedule, and audit inconsistencies were fixed.
The complete schedule and hashes are retained in a write-once study freeze. The DeepSeek v2 output
cap was selected after observing v1's ceiling failures; the manuscript labels that run as an
operational repair, retains v1, and does not pool their p-values.

## 7. Related work

SlopCodeBench studies evolving specifications and structural erosion in iterative coding [2].
SWE-EVO provides long-horizon software-evolution tasks [3]. Work auditing SWE-bench correctness
shows that passing validation can conceal behavioral differences [4], motivating our independent
behavior and preservation oracle. SWE-Bench Pro Verified emphasizes leakage and task-quality
auditing [5]. τ-bench studies stateful multi-turn tool interaction and repeated-trial reliability
[6]. AgentDojo and InjecAgent motivate treating repository and tool content as untrusted data
[7,8]. Our narrower focus is the boundary between revised requirements, evidence lineage, and a
user-facing completion decision.

## 8. Conclusion

In a preregistered six-task small-model component study, conjoining a model completion verdict with
revision/artifact binding reduced external false-ready decisions by 20.8 percentage
points over the prespecified externally rejected stratum (`p=0.03125`). The same mechanism reduced
externally correct coverage by 20.8 points and failed the predeclared usefulness guardrail. Binding
checks reliably identify stale evidence but cannot identify current-bound semantic errors; they may
also reject correct work until evidence is refreshed. The appropriate conclusion is a measured
trade-off, not a universal improvement.

An audited DeepSeek V4 Pro operational repair reproduced the directional result with lower
attrition and a larger −37.5-point false-ready difference (`p=0.03125`), while losing 50 points of
correct coverage. This is cross-model component evidence, not independent confirmation or
cross-repository generalization.

A repository-disjoint end-to-end extension is now frozen: 30 SWE-bench-Live tasks across 10
public repositories, B/C alignment arms, static and two-phase correction variants, and 120 scheduled
sessions for the selected DeepSeek model. It is not reported as a result here because the final
x86_64 hidden-oracle run is an external execution gate; the local Apple Silicon host cannot run all
native x86_64 image tests reliably. The frozen plan, bundle, runner, and analysis artifacts are in
[`repository-completion-swebench-live-v1-plan.md`](repository-completion-swebench-live-v1-plan.md)
and [`swebench-live-v1-model-freeze.md`](swebench-live-v1-model-freeze.md). The extension must retain
the frozen failure handling and external oracle rather than treating this component result as
coding-agent efficacy.

## Data Availability

An anonymized replication package accompanies the submission as supplementary material. It
contains the preregistration, six-task dataset, external Rust oracle harnesses, write-once
288-call study freeze, both DeepSeek follow-up freezes, model-specific manifests, append-only
events, results, summaries, independent audits, and reproduction instructions. Local model weights
are referenced by immutable Ollama digests but are not redistributed; the remote model is identified
by provider alias and run date. The package is scanned for local paths, repository identity,
machine identifiers, and credentials before release. A persistent public URL will be added after
acceptance to preserve double anonymity.

That scan triggered one post-run security redaction: a credential-shaped runner unit-test literal
was replaced by a synthetic token. The original pre-call hash remains in the freezes, and a
declared-redaction audit verifies that requests, events, results, and analyses did not change.

## AI-Assisted Research Disclosure

AI assistants were used to help design adversarial candidates, implement and review the research
harness, execute local experiments, and edit the manuscript. All model-generated protocol and code
changes were inspected before the study freeze. External labels were produced by executable Rust
oracles and deterministic integrity checks, and reported statistics were recomputed from raw
artifacts. AI assistance did not fabricate observations, citations, or missing runs.

## References

1. [ASE 2026 Research Papers CFP](https://conf.researchr.org/track/ase-2026/ase-2026-research-track) (venue policy reference, not research evidence).
2. Orlanski et al., [SlopCodeBench](https://arxiv.org/abs/2603.24755).
3. Le et al., [SWE-EVO](https://arxiv.org/abs/2512.18470).
4. Wang, Pradel, and Liu, [Are “Solved Issues” in SWE-bench Really Solved Correctly?](https://software-lab.org/publications/icse2026_SWE-bench-correctness.pdf).
5. Zheng et al., [SWE-Bench Pro Verified](https://arxiv.org/abs/2609.08149).
6. Yao et al., [τ-bench](https://openreview.net/pdf?id=roNSXZpUDN).
7. Debenedetti et al., [AgentDojo](https://proceedings.neurips.cc/paper_files/paper/2024/file/97091a5177d8dc64b1da8bf3e1f6fb54-Paper-Datasets_and_Benchmarks_Track.pdf).
8. Zhan et al., [InjecAgent](https://aclanthology.org/2024.findings-acl.624/).
