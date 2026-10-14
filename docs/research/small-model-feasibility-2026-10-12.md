# 小模型 feasibility 与 1B secondary 审计（2026-10-12）

本报告只整理可复核的摘要，不替代 raw evidence，也不把 feasibility 或 1B secondary 结果写成主研究显著性结论。新建的脱敏索引见 [`summary.json`](evidence/2026-10-12/small-model-feasibility/summary.json)。其中不包含原始 prompt、context、模型响应、token ID/用量、数据库内容、绝对临时路径或凭据。

## 1. 三次 config-default smoke

三次运行的现有摘要文件都显示：`config-default` 仅满足 1/2 个 benchmark requirement；没有 user-visible completion claim；delivery gate 从未运行；errors 为空；没有成功完成或外部 oracle 的证据。因此这些记录只能说明“尝试过小模型 feasibility”，不能证明通过了交付、门禁或仓库修改。

| 运行 | cap（运行元数据） | 摘要时间 | elapsed | claim | gate / oracle | requirements | 终态 | provenance |
|---|---:|---|---:|---|---|---|---|---|
| `small-model-smoke` | 16,384（默认） | 14:12:15 -04:00 | 313 s | no | never ran / not run | 1/2 | release bar not met | source、binary、model digest 未记录 |
| `small-model-smoke-2` | 2,048（临时） | 14:27:17 -04:00 | 307 s | no | never ran / not run | 1/2 | release bar not met | source、binary、model digest 未记录 |
| `small-model-smoke-3` | 512（临时） | 14:34:34 -04:00 | 308 s | no | never ran / not run | 1/2 | release bar not met | source、binary、model digest 未记录 |

cap 顺序是 16,384 默认值、2,048 临时探针、512 临时探针。后两次 cap 变化已撤回，只能作为 feasibility 信息，不能进入主研究、停止规则、统计检验或显著性叙事。smoke 摘要没有逐次写入 cap，因此 cap provenance 保留为运行元数据；同样，相关但未绑定到这三次运行的其他 manifest digest 不被借用。

这三次 artifact 没有 source/binary/model digest，故无法从 smoke 本身重建精确的源码、binary 或模型版本。没有把其他日期或其他运行的 digest 填入，避免制造错误 provenance。

## 2. 新鲜 1B qualification snapshot

当前 binary、Ollama 0.4.2、local-only empty environment 的 1B qualification 为 **3/7 checks passed，exit 1，not qualified**。摘要指标为 accuracy `0.428571`、mean latency `1359 ms`、TPS `1.3647`、context `4096`，recommended roles 为空。4 个失败类别为 multi-file、patch、context、test-failure；这里不复制测试中具体期望/输出字符串。

该 snapshot 的可复核 digest 为：model `baf6a787…`（完整值在脱敏 JSON 中）、binary `8fa2104a…`、provider source `6e1b044d…`。运行时仓库为 dirty，因此它不是 clean-release qualification；它只支持“当前本机小模型尚未 qualified”的结论。

## 3. 1B v3 secondary 独立复算

审计对象是 [`manifest.json`](evidence/2026-10-12/local-review-v3-llama3.2-1b/manifest.json)、[`results.json`](evidence/2026-10-12/local-review-v3-llama3.2-1b/results.json) 和 [`summary.json`](evidence/2026-10-12/local-review-v3-llama3.2-1b/summary.json)。复算结果如下：

- 64/64 rows 为 `ok`；64 request 与 64 completed response events；32 traces、16 个 base-task clusters；两 seed（17、29）各 32 行。
- 期望标签为 READY 32、BLOCKED 32；原始观测为 **BLOCKED 60、READY 4**，不是 all BLOCKED。
- 总 accuracy = 32/64 = **0.50**；balanced accuracy = **0.50**。
- 期望 READY 侧：2/32 正确 READY，30/32 wrong-block；期望 BLOCKED 侧：30/32 正确 BLOCKED，2/32 false-ready。
- cluster comparison 为 **16 wins / 0 losses / 0 ties**，exact two-sided sign-test `p = 0.000030517578125`。这是未调整的 secondary diagnostic；它不是主分析，也不应写成模型 efficacy 的显著性。每个 cluster 都是 deterministic wrapper 的 1.0 对模型的 0.5，故该 p-value 在此比较中是结构性退化的；summary 的 `significant` flag 仍为 false。

这里的 1B 结果不能支持真实 coding 或交付能力：`live_coding=false`、`repository_execution=false`，标签来自 authored synthetic v3 fixture，seed 是同一 trace 内的重复试验而非独立 base tasks。`primary_model` 是 3B，故 1B 仅能标为 secondary。

### Provenance 缺口

旧 1B manifest 记录了 model/fixture/script/server/schedule/seed 等字段，但没有单列 v2 dependency/gate/probe hash，也没有独立 audit record。因而即使脚本 hash 可核对，也不能独立重建 exact v2 dependency chain；该缺口必须在论文和 evidence index 中保留，不能静默补齐。

## 4. 最窄可安全 claim 与限制

最窄可写 claim 是：

> 在本次 authored synthetic v3 fixture 上，`llama3.2:1b` 的 64 次判断中 32 次正确（accuracy 0.50；balanced accuracy 0.50），原始输出为 60 BLOCKED / 4 READY。相对于确定性 wrapper 的 16 个 base-task cluster 比较为 16/0/0，未调整 exact sign p 为 0.000030517578125；该结果仅作 secondary calibration diagnostic，不是 confirmatory significance 或 coding-efficacy 证据。

必须保留的限制：smoke 没有 per-run digest；cap 探针已撤回且不属于主研究；1B 未通过本机 qualification；v3 无 live coding/repository execution；16 clusters 很小且 seeds 非独立；标签是合成且 wrapper 比较结构性退化；旧 manifest 缺少 v2 dependency/gate/probe 绑定。任何外部模型、真实仓库、用户任务或生产部署的有效性都不能由这些结果外推。

