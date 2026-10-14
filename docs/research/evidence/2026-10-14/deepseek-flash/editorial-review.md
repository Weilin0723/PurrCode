# DeepSeek Flash editorial review

Editorial assistance only; not study evidence. Suggestions require source and evidence checks.

# Skeptical ASE Research-Track Review

**Recommendation posture:** Reject in current form as a research-track paper. The component is honestly reported and the failure of the guardrail is unusually candid, but the submission as written does not clear the bar for archival acceptance. The five obstacles below are ordered by severity. Fixes that are strictly presentational are separated from those requiring new experiments.

---

## 1. Five most serious acceptance obstacles

### Obstacle 1 — Structural monotonicity makes the primary "finding" partly a definitional artifact

The paper itself concedes (§2.3) that for any fixed model response, `C = M ∧ G` is a subset of `B = M`, so

`Pr(C=READY, Y=incorrect) ≤ Pr(B=READY, Y=incorrect)`

holds *by construction*. A negative risk difference is therefore the only possible outcome of a nontrivial gate. The sign test on six clusters returning `p = 0.03125` is, as the authors admit in §6, the minimum attainable value — 6/6 with no ties at n=6 yields exactly that number. Significance here is not evidence of an interesting effect; it is the arithmetic floor of the test.

This does not mean the direction is uninformative. It means the paper cannot rest its novelty or its headline on "false-ready fell." The contribution has to be reframed around **where** the removals occurred, **what they cost**, and **what class of defect is invisible to the gate** — all of which the paper actually measures (§4.3, §4.4). The abstract and contributions currently front-load the direction and bury the boundary.

*Fixable by presentation + reanalysis framing. No new experiments required to fix this, but the paper must stop treating the sign test as the load-bearing result.*

### Obstacle 2 — The preregistered usefulness guardrail failed, and by the paper's own criterion the intervention does not earn an unqualified method-win

The predeclared margin was ≤5 pp loss in correct delivery. Observed losses: −20.8 pp (local 3B), −45.8 pp (1B rubric), −50 pp (DeepSeek v2). Every arm that produced a valid inferential contrast failed the guardrail. The paper says this is "the main result" (§Abstract) and "substantively useful" (§5), and to the authors' credit it is not hidden. But the framing throughout — "significantly reduced false-ready risk," "meets the preregistered statistical criterion," "reproduced the directional result" — reads as salvage. A reviewer will read the abstract and conclude the paper is selling a result that its own primary usefulness gate disqualified.

The deeper problem: after observing the guardrail failure on local 3B, the authors ran a second model (DeepSeek) and observed the same failure. That is a *negative* result about the intervention as designed, not a positive result with a caveat. A strong ASE paper could be built on "binding is a stale-evidence filter that is not a free improvement and here is the cost structure," but it requires the paper to *be* that paper rather than a method-win paper with a disclaimer.

*Fixable by framing, but the framing change is severe enough that it reshapes the abstract, intro, contributions, and conclusion.*

### Obstacle 3 — The evaluation cannot support generalization beyond a six-task authored fixture, and the paper acknowledges this but still asks the reader to accept the abstraction

Six tasks, one authored Rust fixture, six systematic counterexamples per task, all labels from a single harness. The paper honestly lists external validity as a threat (§6). But the abstraction being defended — "revision- and artifact-bound completion" — is a general mechanism claim, and six hand-built tasks with hand-built counterexamples are the weakest possible evidence for it. The candidate strata are not sampled from real defect distributions; they are adversarial constructions chosen by the authors. This means:

- The gate's 50% snapshot acceptance (§4.4) is a property of the authors' construction, not of real repositories.
- The guardrail failure magnitude is not estimable; it depends entirely on how many correct-but-stale snapshots the authors chose to include (one per task, here).
- The DeepSeek replication is on the *same fixture*, so it is a model-robustness check, not a task-robustness check.

The frozen SWE-bench-Live extension (§8) would address this, but it is explicitly not reported. A component paper can be small, but it cannot be small *and* authored *and* single-fixture *and* assert a general control-plane principle.

*Requires new experiments to fix substantively. The SWE-bench-Live extension is necessary, not optional, for an archival claim.*

### Obstacle 4 — Outcome-aware replication, mutable aliases, and invalid partial live evidence together undermine the "cross-model" claim

Three problems compound:

1. **Outcome-aware repair.** The DeepSeek v1 → v2 transition changed the output cap after observing v1's ceiling failures. The paper labels this "transparent" and "not independent confirmation" (§3.4, §4.5), which is correct, but it also uses v2 as the second model's evidence of the filtering effect. A replication that was triggered by observing an outcome, even an outcome the authors argue is orthogonal to the effect, is not a replication in the ordinary sense. It is a *second operational run on the same fixture*.
2. **Mutable alias.** `deepseek-v4-pro` is a dated service alias, not pinned weights (§3.4, §6). The paper cannot claim the DeepSeek result is reproducible in the sense the local digest-pinned models are. Any future reader running the same study may get a different model.
3. **Invalid partial live evidence.** §4.6 reports four downloaded cells from the SWE-bench-Live run with failed sessions, zero output tokens, empty patches, and unknown oracles. This is correctly described as unable to establish a failure rate — but it also means the paper cites a live extension as "frozen" and then reports only broken cells. The extension is neither a result nor a clean negative; it is an unqualified execution gate. Including the row in the evidence ledger table (§4.6) invites the reader to weight it, when it carries no evidential weight.

*Fixable by (a) demoting DeepSeek to "an additional model-external observation" rather than a replication, (b) removing the SWE-bench-Live row from the ledger or clearly marking it as a plan, not a study, and (c) either dropping or fully reporting the live run. (a) and (b) are presentation. (c) requires new execution.*

### Obstacle 5 — Attrition and the all-READY pattern mean the local study measured transport reliability more than reviewer behavior

64/144 valid for 3B (44.4%), 93/144 for 1B (64.6%). All 80 3B invalid cells were 30-second timeouts; all 51 1B invalid cells were truncated JSON. Among valid responses, **every single one was READY** (§4.1). That means:

- The B condition on the local primary model is effectively "READY unless the transport failed."
- The gate's effect is measured against a model that provided essentially no signal.
- ITT handling (invalid → non-ready) means B's false-ready rate is *suppressed* by timeouts, which mechanically narrows the gap C can produce. This is not wrong, but it means the reported −20.8 pp is not a clean estimate of gate filtering on a functioning reviewer.

The DeepSeek follow-up was meant to fix this (143/144 valid). But because DeepSeek is outcome-aware and alias-mutable (Obstacle 4), it does not carry the confirmatory weight the local study was supposed to carry. So the paper is left with: a local study whose model is too weak to be a reviewer, and a remote study whose protocol is too confounded to be confirmatory.

*Requires new experiments: a local model that passes provider qualification for the review role, or a remote model run under a frozen non-outcome-aware protocol.*

---

## 2. Fixable presentation issues vs. missing experiments

**Fixable by presentation / reframing only:**

- Abstract and contributions currently lead with the false-ready reduction and demote the guardrail failure. This inverts the paper's own main result (§Abstract, §5). Reorder.
- The sign test at n=6 with 6/6 wins is presented as meeting "the preregistered statistical criterion" (§4.2). It should be presented as the minimum attainable value of the test, not as a substantive significance result. The paper already says this in §6 but not where the result is reported.
- DeepSeek v2 is called a "replication" in several places (§4.5 heading, §5, §8). It should be called what §3.4 already says it is: an outcome-aware operational follow-up.
- The SWE-bench-Live row in the ledger table (§4.6) should be removed or explicitly marked "plan, not result."
- Contribution 4 ("statistically significant false-ready reduction together with a failed coverage guardrail") should be rewritten to lead with the guardrail failure.
- The §4.6 operator-audit paragraph reports four cells and is labeled unable to establish a failure rate — correct, but it should either be expanded to a usable sample or moved to a footnote in the threats section.
- "Cross-model" in §8 and §Abstract should be softened to "model-external" or "a second model."
- Contribution 5 calls the follow-up a "replication." Same fix.

**Requires new experiments:**

- Any claim of generalization beyond the six-task authored fixture. The SWE-bench-Live extension is the intended fix and is not reported.
- A clean confirmatory contrast on a model that (a) passes qualification for the review role and (b) is run under a protocol frozen before observing its outcomes. Neither the local models nor the outcome-aware DeepSeek run satisfy both.
- A guardrail evaluation whose magnitude is not determined by the authors' choice of how many correct-but-stale snapshots to include. This requires either a sampled candidate distribution or a sensitivity analysis over the correct-but-stale proportion — neither is present.
- Any claim that evidence refresh (as opposed to evidence binding) changes the trade-off. §1 RQ3 / §5 name this as the next intervention; it is unmeasured.

---

## 3. Replacement wording supported only by existing evidence

The following replacements remove unsupported claims (generalization, replication, method-win, significance-as-substance) and retain only what the frozen evidence shows.

### Replacement abstract

> Coding agents can claim completion after a request changes while relying on evidence produced for an obsolete requirement or artifact. We study a narrow control-plane intervention: conjoin a model's completion verdict with a deterministic gate that requires current requirement, artifact, and evidence bindings. The gate is not a semantic oracle; it checks only whether recorded evidence is current and bound to the reviewed artifact. Because conjunction is monotone, any nontrivial gate can only remove model READY decisions, so the empirical questions are the magnitude of removal, the strata in which it occurs, and the availability cost.
>
> We preregistered a repository-snapshot component study before any model generation. Six authored Rust base tasks each contributed six immutable candidate snapshots: an unchanged baseline, a correct repair, a stale repair, an over-broad repair, a test-tampering repair, and a behaviorally correct repair with stale evidence. Independent Rust behavior oracles, preservation checks, and test-integrity checks produced external labels without entering model prompts or authorization. Local Llama 3.2 3B was the primary reviewer and Llama 3.2 1B secondary. Two prompt conditions and two repeated seeds produced 288 scheduled calls. The primary contrast was model verdict alone (B) versus the same verdict conjoined with the deterministic gate (C), clustered over the six base tasks.
>
> In the preregistered 3B revision-rubric condition, C reduced external false-ready decisions from 23/48 (47.9%) to 13/48 (27.1%). All six task clusters favored C; the preregistered two-sided exact sign test returned its minimum attainable value (`p=0.03125` at n=6, 6/6 with no ties) and is therefore reported as a direction indicator rather than a substantive significance result. The predeclared usefulness guardrail failed: coverage of externally correct snapshots fell from 10/24 (41.7%) to 5/24 (20.8%), exceeding the 5-point margin by 15.8 points. The removal was concentrated in the stale strata; current-bound over-broad and test-tampering snapshots remained false-ready, and the correct-but-stale snapshots were blocked. Strict-output attrition was high (64/144 valid at 3B; 93/144 at 1B), and every strictly valid local response declared READY, so the local B condition carries little reviewer signal. The bounded claim supported by this evidence is that revision binding acted as a stale-evidence filter on an authored small-model fixture and did not act as a semantic correctness oracle or a free improvement.
>
> We additionally observed the same contrast on the `deepseek-v4-pro` service alias using the same fixture and schedule. A first frozen run was operationally inconclusive (26/144 responses hit a 128-token ceiling). An outcome-aware second run changed only that ceiling and reran all 144 cells; 143/144 responses were valid. In its primary condition, false-ready fell from 30/48 (62.5%) to 12/48 (25.0%) and correctness coverage fell from 24/24 to 12/24, again failing the guardrail. Because the repair was triggered by observing v1 and the model is identified by a mutable dated alias rather than pinned weights, we report this as an additional model-external observation on the same fixture, not as independent confirmation, cross-model replication, or cross-repository generalization.

### Replacement contributions

> 1. A revision- and artifact-bound completion rule that is explicitly separated from a semantic oracle, and an explicit statement of the monotonicity property that bounds what such a rule can establish.
> 2. A frozen six-task, six-candidate authored fixture that includes both current-but-wrong and correct-but-stale counterexamples, with external labels produced by independent Rust behavior, preservation, and integrity checks that do not enter model context.
> 3. A preregistered 288-call small-model study with task-clustered inference, intention-to-treat failure handling, write-once freeze records, and independent recomputation. We report the sign test at its minimum attainable value rather than as a substantive significance result.
> 4. Evidence that on this fixture the binding gate acted as a stale-evidence filter and failed its predeclared usefulness guardrail by a wide margin, with the removal concentrated in the stale strata and current-bound semantic defects left untouched. We report the guardrail failure as the primary result rather than as a caveat on a safety gain.
> 5. An additional model-external observation on a mutable service alias, explicitly labeled as an outcome-aware operational follow-up rather than a replication, retaining the failed first remote run and not pooling its inferential outputs.
> 6. A frozen, not-yet-executed cross-repository plan (30 SWE-bench-Live tasks, 120 scheduled sessions) together with a partial operator audit of four cells that establishes only that workflow success is insufficient evidence of experimental validity. No cross-repository result is claimed.

---

## 4. One-line summary for the authors

The paper's own §2.3 and §4.3 are the honest paper. The current abstract and contributions are a different, more conventional paper that the evidence does not support. Rewrite toward the former, drop the word "replication," demote the sign test to its actual role, and either execute the SWE-bench-Live extension or drop it from the ledger — and the submission becomes a defensible negative-result component paper, which is a publishable thing at ASE if it is framed as such.
