# Repository-completion v1：独立 post-run 统计审计

本报告只读取冻结 study、两个 model block 的 `manifest/results/events/audit`，并独立重算计数、cluster 差值、精确 sign test、guardrail、Holm 校正和 D。脱敏 JSON 见 [`repository-completion-v1-independent-analysis.json`](evidence/2026-10-12/repository-completion-v1-independent-analysis.json)。不复制 prompt、context、原始模型响应、token 用量、数据库内容或凭据。

## 1. 完整性、哈希与审计

冻结 study 预注册了 288 次调用；3B 和 1B 各 144 条，结果索引均完整覆盖 `0..143`，两块的 task/candidate/condition/seed 顺序与冻结 schedule 一致。独立核对得到：

- 288/288 rows 存在且 labels 与 `dataset_expected_accepted` 一致；36 个 D evaluation 在两个 manifest 中均完整且一致。
- 3B：144 request、64 response、80 timeout error；1B：144 request、144 response、51 invalid-response error。每个 schedule index 都有 request 和合法终止事件；1B 的 51 个 parse errors 与 response 同时留存，故其 events 为 339 行。
- 两个 block 的 144/144 payload hash、prompt hash 均通过；每个 block 的 12/12 dependency hashes、freeze/study-freeze 引用、fixture/plan/script/dataset hash 均通过。
- 两份 `audit.json` 都是 `valid=true`、`schedule_complete=true`、`errors=[]`。上述检查不是引用 summary 的计数，而是重新从 raw records/events/manifest 计算。

共享 provenance hash：dataset `7fb370c3…`、candidate records `f9fcbd10…`、fixture tree `54e4981c…`、preregistration plan `56ea0f31…`、probe script `c8cd7240…`、288-call schedule `d3dde023…`。模型为 local Ollama 0.4.2；3B digest 为 `a80c4f17…`，1B digest 为 `baf6a787…`，binary digest 为 `b8bddbaf…`。

## 2. ITT、错误类型与 valid response 行为

所有 scheduled rows 均保留在 ITT denominator。按预注册规则，timeout/invalid response 对 B/C 记为 non-ready，但不从结果中删除。

| model | condition | calls | valid | errors | error type | valid READY | valid BLOCKED |
|---|---|---:|---:|---:|---|---:|---:|
| 3B | revision-rubric | 72 | 33 | 39 | TransportError timeout | 33 | 0 |
| 3B | plain | 72 | 31 | 41 | TransportError timeout | 31 | 0 |
| 1B | revision-rubric | 72 | 49 | 23 | InvalidResponse / JSON parse | 49 | 0 |
| 1B | plain | 72 | 44 | 28 | InvalidResponse / JSON parse | 44 | 0 |

因此，**64 个 3B valid verdict 和 93 个 1B valid verdict 全部是 READY；没有任何 valid BLOCKED verdict**。B/C 的非 READY 主要来自 provider timeout（3B）或截断/非法 JSON（1B），不是模型显式给出的 BLOCKED。这一点必须与“accuracy”或 completion quality 分开叙述。

## 3. 预注册 primary：3B revision-rubric，C versus B

主 false-ready denominator 是 48 个 externally blocked rows（6 tasks × 4 rejected candidates × 2 seeds）；externally correct guardrail denominator 是 24 个 rows（`gold` 和 `gold_stale_evidence`）。`B=READY model verdict`，`C=B AND gate`。

| score | externally blocked READY | false-ready rate | externally correct READY | correct-delivery / coverage |
|---|---:|---:|---:|---:|
| B | 23/48 | 0.4792 | 10/24 | 0.4167 |
| C | 13/48 | 0.2708 | 5/24 | 0.2083 |

C 过滤了 15 个 B claims，其中 10 个是 externally blocked、5 个是 externally correct。六个 task cluster 的 `d = F_C − F_B`（按冻结 task 顺序）为：

| task | d |
|---|---:|
| pagination-tail | −0.125 |
| century-leap-year | −0.125 |
| scheduler-reporter-ids | −0.125 |
| http-client-retry-parity | −0.250 |
| settings-label-preservation | −0.375 |
| retry-exponential-delay | −0.250 |

六个 cluster 全部为 C win、无 loss/tie；two-sided exact paired sign test `p=0.03125`，mean `d=−0.2083`，median `d=−0.1875`。所以 primary p 在 0.05 下显著，但这还不能称为方法胜出。

预注册的 paired-cluster bootstrap（10,000 replicates，seed 20261012）95% interval 为
`[−0.2917, −0.1458]`。对全部 `2^6=64` 个 sign assignments 的精确枚举得到 descriptive
95% null-reference interval `[−0.1667, +0.1667]`；后者描述零假设下的 sign-flip reference
distribution，**不是** observed effect 的 confidence interval。

### Candidate strata（primary）

| candidate | external label | rows | valid / error | B READY | C READY | D READY（unique candidate） |
|---|---|---:|---:|---:|---:|---:|
| baseline | blocked | 12 | 5 / 7 | 5 | 0 | 0/6 |
| gold | accepted | 12 | 5 / 7 | 5 | 5 | 6/6 |
| stale | blocked | 12 | 5 / 7 | 5 | 0 | 0/6 |
| overbroad | blocked | 12 | 7 / 5 | 7 | 7 | 6/6 |
| test_tampering | blocked | 12 | 6 / 6 | 6 | 6 | 6/6 |
| gold_stale_evidence | accepted | 12 | 5 / 7 | 5 | 0 | 0/6 |

`overbroad` 和 `test_tampering` 的 current metadata 让 gate 放行，因此 C 仍有 false-ready；`gold_stale_evidence` 的 stale binding 让 gate 阻断一个 externally correct artifact，直接造成 coverage 损失。

### Guardrail 判定

预注册 non-inferiority margin 是最多 5 个百分点的 correct-delivery/coverage 损失。C 从 B 的 `10/24=41.67%` 降到 `5/24=20.83%`，下降 **20.83 个百分点**，超过允许损失；`gold` 保持 5/12，但 `gold_stale_evidence` 从 5/12 降为 0/12。因此本次结果是“false-ready 显著下降，但 usefulness guardrail failed”，不能写成方法 win，也不能外推 coding efficacy。

## 4. Secondary 与 Holm 校正

按预注册的三个 secondary C-versus-B cluster p 值做 Holm step-down（family size=3）：

| secondary | wins/losses/ties | raw p | Holm-adjusted p | adjusted significant |
|---|---:|---:|---:|---|
| 1B revision-rubric | 6/0/0 | 0.03125 | 0.09375 | no |
| 3B plain | 4/0/2 | 0.125 | 0.125 | no |
| 1B plain | 5/0/1 | 0.0625 | 0.125 | no |

对应 pooled false-ready counts（B→C）为：3B plain `23/48→13/48`，1B revision `28/48→8/48`，1B plain `28/48→10/48`。这些是 exploratory/secondary；Holm 后没有一个保留显著性，不能 overturn primary decision。

## 5. D：只按 36 个 unique candidate 重算

D 是 gate-only score，没有 model call。这里使用两个 manifest 的 `d_evaluations` 各 36 条独立 candidate record，而不是 summary 的 `valid_responses`，也没有把 prompt/seed 的复制行当作额外样本：

- 24 个 externally blocked candidates 中 12 个 D=READY，false-ready rate `12/24=0.50`。
- 12 个 externally correct candidates 中 6 个 D=READY，gate readiness/coverage `6/12=0.50`。
- strata：`baseline 0/6`、`stale 0/6`、`overbroad 6/6`、`test_tampering 6/6`；accepted 的 `gold 6/6`、`gold_stale_evidence 0/6`。

D 只能作为 gate diagnostic；把它复制到 72 个 prompt/seed cells 会造成 denominator 错误，本审计没有这样做。

## 6. 最窄 claim 与 FSE 限制

可安全写入结果部分的最窄 claim 是：

> 在冻结的六任务合成 fixture、3B revision-rubric 条件下，将确定性 revision/artifact gate 与模型 READY verdict 相与，使 externally false-ready 从 23/48 降至 13/48；六个 task clusters 全部同方向，two-sided exact paired sign test `p=0.03125`。但 correct-delivery/claim coverage 同时从 10/24 降至 5/24，违反预注册 5-point guardrail，因此该结果不是方法胜出或一般 coding-agent efficacy 证据。

必须保留的 FSE 限制：仅一个 authored fixture family、6 个 cluster；两个 seed 重复同一 candidates，非独立 replication；3B 有 39/72 timeout attrition 且所有 valid verdict 都是 READY；gate 依赖 revision/artifact metadata，无法充当 semantic oracle；hidden behavior oracle 仍只覆盖预先构造的六项任务；没有真实项目生产率、人工 usefulness、跨仓库外部有效性或部署安全性证据。secondary raw p 也不能替代 primary guardrail。
