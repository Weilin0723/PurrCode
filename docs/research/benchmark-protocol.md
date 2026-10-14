# FSE research benchmark protocol — draft v0.1

Date: 2026-10-10. Status: study design and feasibility work, not a completed experiment.
See `preflight-2026-10-10.md` for executed checks and limitations.
FSE 2027 new research-paper submissions closed on October 3 AoE; this plan targets a later cycle.

## Recommended question

Can revision-bound completion evidence reduce externally incorrect completion claims after a
user changes requirements, without materially reducing correct delivery or increasing cost?

Working title: **When Requirements Change: Evidence-Gated Completion for Coding Agents**.
PurrCode is the implementation vehicle. The contribution must be a falsifiable method and
independent evaluation, not the size of its IDE, number of agents, or a new collection of prompts.
Novelty is provisional until the related-work comparison in `fse-positioning.md` is complete.

The proposed mechanism binds requirement verification to the current requirement revision and
relevant artifact state. A correction invalidates affected evidence; a deterministic reducer
permits a completion transition only with current evidence. Existing v1.5 facilities provide a
starting point, but selective invalidation and all ablation configurations require code tracing
and tests before they may be claimed as implemented.

## Questions and decision rule

- RQ1: How often do agents claim completion while an independent oracle rejects the latest request?
- RQ2: Does the method reduce this error relative to equally budgeted prompting and review?
- RQ3: How much is attributable to revision binding versus additional review or repair tokens?
- RQ4: What are the costs in successful deliveries, elapsed time, tokens, and unnecessary blocking?

Primary endpoint: false completion among all scheduled, evaluable task runs. Also report false
completion among completion claims, claim coverage, external correctness, and correct delivery
(correct AND claimed complete). A system that never finishes cannot win on error rate alone.
Predeclare a 5-percentage-point correct-delivery non-inferiority margin before confirmatory runs;
use pilot results to estimate precision and sample size, never to change the margin after seeing
held-out outcomes. If intervals cannot resolve the tradeoff, report inconclusive evidence.

## Experiment arms

| Arm | Behavior | Implementation status |
| --- | --- | --- |
| A: prompt checklist | Same base coding loop, latest request/checklist in context, ordinary completion | Research adapter not implemented |
| B: review and repair | Same loop plus independent review and bounded repair, no revision-bound completion evidence | Research adapter not implemented |
| C: full method | Current contract, current evidence, deterministic completion reducer | Existing runtime; full intervention path needs audit |
| D: no revision invalidation | Same as C, retain stale review evidence after correction | Research-only ablation not implemented |

Keep PawGate, exact-action durable authorization, Claw recheck, credential scrubbing, and isolation
identical in every arm. Never create a production execution bypass for a research baseline.
Do not confuse delivery-policy ablation with tool-authorization ablation. Implement research arms
in isolated fixtures/adapters and explicitly label simulation evidence if native execution is absent.

Use the same model, tool surface, initial snapshot, task messages, maximum output tokens,
whole-task wall deadline, and total token budget per matched comparison. Count review, contract
compilation, repair, and tool-wait time. A shared token ceiling, not identical realized token use,
defines budget parity. Report budget-response curves if the method uses more tokens.

## Dataset and staging

1. **Now: instrumentation.** Existing 20-task alignment catalog is development/regression data.
   Preserve successes, failures, unavailable providers, and missing usage separately. It is not a
   held-out research test set because the implementation has already been tuned against it.
2. **Pilot: proposed 36 runs.** Six newly curated tasks × A/B/C × one qualified local model × two
   repeats. Include scope preservation, changed behavior, API compatibility, correction after
   validation, redundant correction, and a benign static control. At 600 seconds/run the agent
   time cap is six hours, plus environment setup and independent validation. This is a budget
   ceiling, not a forecast. These tasks are not yet curated or run.
3. **Confirmatory: candidate 1,440 runs.** Thirty base tasks from at least six real repositories,
   each with a static-final-request and a mid-run-correction variant (60 cases), × four arms × two
   model families × three repeats. At 600 seconds/run the agent time cap is 240 hours, plus setup
   and validation. Choose final N using pilot variance, power/precision, and available compute;
   freeze it before looking at held-out results. This scale is proposed, not a claimed dataset.

Use repository-disjoint development and held-out splits. Pair variants within each source task;
do not call them independent samples. Capture source commit, license, issue/provenance, explicit
requirement quotes, initial request, correction, injection rule, gold patch, and oracle version.
Have two people independently validate request/oracle consistency and adjudicate disagreements.
Model-generated labels alone are insufficient. Do not add requirements only through hidden tests.

## Correction delivery and fairness

Trigger the correction at a scaffold-neutral observable milestone (e.g. first successful edit or
first validation completion), not the presence of a PurrCode contract. Record the exact trigger,
message acknowledgement, and artifact revision. Predeclare a fallback if the trigger is never
reached. A run that finishes before an intended correction is delivered is not a successful
correction trial: report trigger failure separately and retain it in scheduled-run accounting.

Static controls receive the complete final request initially. Correction trials receive only
information available at each turn. The task dataset must remain solvable after correction;
report irreversibility explicitly instead of forcing impossible repair.

## Independent oracle

Use hidden behavioral tests plus explicit preservation checks outside agent-writable paths.
Run them against an immutable snapshot after the worker is confirmed stopped. Isolate tests
from host credentials and network. Gold patch must pass; unchanged tree and deliberately stale,
partial, overbroad, and test-tampering patches must fail the appropriate checks. Check that the
oracle accepts alternative valid implementations, not only the gold diff. Hash oracle inputs.

The existing Contains/Lacks/Exists checks are only proxy labels. They can accept comments or
nonfunctional code; repository tests can be edited by the agent. Passing these checks cannot be
presented as semantic correctness. The new `scripts/research_metrics.py` separates external-check
outcomes from gate consistency but does not repair oracle quality.

## Recording and analysis

Store one append-only result per attempted run, including failures, before proceeding. Record:
run ID; task and base-task ID; arm; repetition; model ID and digest; provider/adapter version;
decoding settings and seed support; source and dirty-file hashes; prompt hashes; budgets; complete
trace; correction delivery/acknowledgement; terminal reason; claimed completion; immutable final
patch hash; per-oracle verdict; token usage or null; latency; validation timeout; infrastructure
failure. Keep credentials out of config snapshots, process environments, traces, and artifacts.

Report infrastructure/qualification failures separately, with scheduled-run denominators and
attrition by arm. Do not silently drop timeouts, replace runs, or choose best-of-N. Model tool
loops are capped by the actual runtime (`MAX_AUTONOMOUS_ITERATIONS` is currently 32); fixture
validation timeout and whole-agent deadline are distinct budgets.

Analyze paired task differences and confidence intervals with resampling clustered by base task
and repository. Repeated seeds do not increase the number of independent tasks. Correct for
multiple secondary comparisons; predeclare the primary contrast C versus B. Report per-model,
per-repository, static/correction, and requirement-class results. With only six repository
clusters, generalization uncertainty will remain substantial. Per-report Wilson intervals from
the lightweight analysis script are descriptive diagnostics, not confirmatory paired inference.

## Immediate engineering order

- [x] Preserve the dirty working tree and record code/binary provenance for local preflight.
- [x] Start real local provider qualification using isolated local-only configuration.
- [x] Finish preflight evidence and classify incomplete/interrupted runs without a model verdict.
- [x] Complete independent score summarizer and six regression tests.
- [ ] Fix research-runner lifecycle before live comparisons: unique run directories; bounded HTTP;
      cancel and await stopped on timeout; snapshot final artifacts; persist per-run partial
      results; check correction HTTP responses; one isolated validation per snapshot.
- [ ] Curate six pilot tasks with reviewed behavioral oracles.
- [ ] Implement and validate matched research arms; never change tool authorization semantics.
- [ ] Run pilot, freeze study design, then fund/schedule the confirmatory run.

No RC milestone is marked complete by this work. No autonomous dogfooding or post-Phase-5 release
qualification is implied. The research plan is separate from the RC qualification checklist.

## Primary related-work starting points

- [SlopCodeBench](https://arxiv.org/abs/2603.24755) already studies iterative development under
  evolving specifications. “Multi-turn coding” alone is not a novelty claim.
- [SWE-EVO](https://arxiv.org/abs/2512.18470) studies long-horizon software evolution; distinguish
  requirement-revision evidence and completion decisions from broad evolution capability.
- [Are Solved Issues in SWE-bench Really Solved Correctly?](https://software-lab.org/publications/icse2026_SWE-bench-correctness.pdf)
  motivates scrutiny of test-passing patches; do not claim that test pass equals semantic success.
- [SWE-bench Live](https://arxiv.org/abs/2505.23419) is a candidate source of fresh, executable tasks,
  subject to task-level license, scope, and oracle audit. No external tasks have been imported yet.
