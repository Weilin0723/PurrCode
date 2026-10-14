# DeepSeek V4 Pro repository-completion replication: frozen analysis plan

Date: 2026-10-13  
Study ID: `repository-completion-deepseek-v1`  
Status: preregistration text; the machine-readable freeze must be written and audited before the
first repository-completion model request.

## Purpose and separation from the small-model study

This study is a model-external replication of the previously frozen repository-completion
component experiment. It does not replace, pool with, or retroactively change the confirmatory
3B result. Its purpose is to test whether the observed reduction in false-ready decisions and the
associated correct-coverage loss persist for a qualified remote model.

Before this plan was written, `deepseek/deepseek-v4-pro` passed the repository's generic seven-case
provider qualification suite (7/7; mean latency 1,136 ms). Qualification did not use any task,
candidate, prompt, or outcome from this study and is not part of its analysis. No
repository-completion prompt may be sent until the freeze, request bundle, and their hashes exist.

## Fixed model and transport

- Provider: the official DeepSeek API at `https://api.deepseek.com`.
- Requested model alias: `deepseek-v4-pro`.
- Official catalog state consulted on 2026-10-13: the alias was documented as DeepSeek V4 Pro
  (V4-Pro-0813). The service does not expose an immutable local weight digest, so this study will
  describe the model as an API alias observed at a dated run, not as a cryptographically pinned
  snapshot.
- Wire protocol: OpenAI-compatible streaming chat completions through the repository's existing
  Rust `purrcode-provider-gateway` credential boundary.
- Output mode: schema-constrained JSON. On the official DeepSeek host, the gateway explicitly
  disables thinking for structured calls and requests JSON output.
- Maximum output: 128 tokens.
- Provider defaults: non-thinking sampling defaults apply. The research runner does not claim that
  DeepSeek supports or received a seed. Schedule labels 17 and 29 are replicate identifiers only.
- Credential handling: the runner resolves `keychain:deepseek` from the config-adjacent credential
  store. The credential must never enter Python, argv, environment variables, prompts, events,
  manifests, archives, or child processes.

The model ID, endpoint host, request-bundle hash, prompt hash, response content, provider usage,
latency, and terminal status are durable evidence. Raw authorization headers are never evidence.

## Frozen material and schedule

The study reuses the existing six-task dataset and externally executed oracles without modifying
the old frozen files. Candidate snapshots and prompts are rebuilt before the freeze and must match
the existing dataset expectations and smoke checks.

- Task clusters: 6.
- Candidates per task: `baseline`, `gold`, `stale`, `overbroad`, `test_tampering`, and
  `gold_stale_evidence`.
- Prompt conditions: `revision_rubric_json` and `plain_json`.
- Replicate labels: 17 and 29.
- Fixed order: condition, dataset task order, candidate order above, then replicate label.
- Total scheduled calls: `2 × 6 × 6 × 2 = 144`.
- Primary condition: `revision_rubric_json`.
- Secondary condition: `plain_json`.

The complete schedule, prompts, prompt hashes, candidate labels, external-oracle labels, dependency
hashes, runner hash, analysis-script hash, and this plan's hash must be recorded before call 0.
Candidate mutation names and external labels never enter prompts.

## Outcomes

For every scheduled row:

- **B (model-only ready):** the terminal response is valid and its verdict is `READY`.
- **C (bound ready):** B is true and the deterministic revision/artifact/evidence gate is `READY`.
- **D (gate-only diagnostic):** the deterministic gate is `READY`, evaluated once per unique
  candidate snapshot rather than once per repeated model call.

An error, timeout, invalid response, or missing terminal response is not silently discarded and is
not replaced. Terminal errors are counted as non-ready in the intent-to-treat table, while
attrition is reported separately. A request recorded as started without a terminal event is
`uncertain`; it is not automatically resent because it may already have reached the provider.

## Confirmatory estimand and inference

The sole confirmatory estimand is the task-cluster mean difference in false-ready rates under the
revision-rubric prompt:

```text
Delta_DS = mean_task(false_ready_C_task - false_ready_B_task)
```

The confirmatory denominator comprises the four externally rejected candidates for each task,
two replicate labels each: 48 scheduled rows total and 8 rows per task cluster.

The confirmatory report must include:

1. all six task-level paired differences;
2. the mean and median paired difference;
3. an exact two-sided paired sign test over non-tied task clusters;
4. a 10,000-replicate paired cluster bootstrap 95% interval using seed `20261013`; and
5. all 64 exact sign-flip assignments as a descriptive 95% null-reference interval, explicitly
   labelled as not being a confidence interval.

Significance uses `alpha = 0.05`. There is no optional stopping, task deletion, candidate deletion,
model substitution, or reclassification based on the result. With six clusters, the minimum
attainable two-sided no-tie sign-test p-value is 0.03125.

## Predeclared usefulness guardrail

Correct coverage is the READY rate over externally accepted candidates in the primary condition:
`gold` and `gold_stale_evidence`, 24 scheduled rows total. The intervention passes the guardrail
only if the C-minus-B coverage difference is no worse than -5 percentage points.

False-ready improvement with a loss beyond 5 points must be described as a safety--availability
trade-off, not as an unconditional method win. Results for `gold` and `gold_stale_evidence` must
also be shown separately.

## Secondary and diagnostic analyses

The following are secondary or descriptive and cannot rescue a failed primary result:

- the same paired analysis for the plain prompt;
- candidate-stratified B and C rates;
- D compared descriptively with B and C on unique candidate snapshots;
- interaction contrasts against the earlier 3B estimates, without pooling the studies;
- invalid response, provider error, timeout, latency, input-token, and output-token distributions;
- exact-response agreement between replicate labels.

If multiple secondary hypothesis tests are reported, their p-values form one Holm-adjusted family.

## Operational validity and stopping

The schedule is complete only when all 144 indices have a durable terminal response or terminal
error. Fifteen or more error/invalid rows (>10%) make the model-efficacy interpretation
operationally inconclusive; the complete intent-to-treat result is still reported. Missing usage
for more than 10% of terminal calls makes cost accounting incomplete and must not be imputed as
zero.

The hard study budget is 150,000 provider-reported input plus output tokens, excluding the generic
qualification suite. A pause is allowed only for billing/usage exhaustion, provider outage,
credential or integrity failure, a reached token ceiling, or user interruption. Resume must use
the identical freeze and may continue only never-started schedule indices. It must not depend on
the observed effect or p-value.

## Claim boundary

This is a six-cluster, single-fixture component replication. Even a significant result supports
only a model-external replication of completion-judgment behavior. It does not establish
cross-repository generalization, semantic correctness of generated patches, or end-to-end coding
agent efficacy. A main-track generalization claim still requires repository-disjoint tasks and an
end-to-end study.
