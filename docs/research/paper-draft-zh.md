# 证据新鲜不等于行为正确：编码 Agent 需求修订完成判定审计

**ASE 主会研究论文工作稿——组件证据完整，端到端资格门仍开放**
**日期：** 2026-10-14
**证据状态：** 本地 288/288 调用完成；DeepSeek 144-call operational repair 通过审计

## 摘要

当用户在编码过程中修订需求时，agent 可能继续依赖旧需求或旧 artifact 上产生的证据并宣布完成。本文研究一个窄而明确的控制面干预：把模型的完成判定与确定性的 request/artifact/evidence binding gate 取交集。该 gate 检查证据的新鲜性与绑定关系，但不是语义正确性 oracle。

我们在任何模型生成调用之前冻结了一个 repository-snapshot 组件研究。6 个 base tasks 各有 6 个不可变候选：baseline、正确修复、stale 修复、over-broad 修复、test-tampering 修复，以及代码正确但 evidence stale 的修复。位于 agent worktree 外部的 Rust 行为 harness、preservation checks 和 test-integrity checks 共同产生外部标签，且标签和 oracle 输出不进入模型 prompt 或授权路径。主模型为本地 Llama 3.2 3B，1B 为 secondary；两个 prompt 条件和两个重复 seed 共形成 288 个固定调用。

在预注册的 3B revision-rubric 条件下，模型单独判定 B 的 external false-ready 为 23/48（47.9%），与 gate 取交集后的 C 为 13/48（27.1%），paired risk difference 为 −20.8 个百分点。6 个 task clusters 全部同向，双侧 exact sign test `p=0.03125`，达到预注册显著性标准。但 usefulness guardrail 失败：externally-correct coverage 从 10/24（41.7%）降至 5/24（20.8%），远超预设最多损失 5pp 的界限。当前绑定但语义错误的 over-broad/test-tampering 候选仍会 false-ready；行为正确但 evidence stale 的候选则被阻断。

由于 C 的接受集合天然包含于 B，C−B 的差不可能为正。预注册 sign test 的数值保留用于审计，但它的符号对称假设不能在这个单向过滤设计中证明方法有效；摘要中的效应大小应作描述性解释，而不能依赖显著性推销方法。

因此，本文支持的最窄结论是：在该人工构造的小模型 fixture 的预注册 externally-rejected 候选总体中，revision binding 减少 false-ready decisions；下降集中于 baseline/stale strata，但没有对 stale-only 子层做独立推断。它不是语义 correctness oracle，也不是无代价改进。主要发现是 safety–availability trade-off，而非“方法全面胜出”。

随后我们用官方 `deepseek-v4-pro` API 做 model-external follow-up。首个冻结运行有 26/144 个响应触及 128-token 上限并形成不完整 JSON，超过 10% attrition 阈值，因此永久标记为 operationally inconclusive。一个 outcome-aware repair 只把上限改为 256，并重新执行全部 144 cells；143/144 输出有效。主条件 false-ready 从 30/48（62.5%）降到 12/48（25.0%），六个 cluster 全部同向（`p=0.03125`，bootstrap 95% CI [−45.8, −29.2]pp）。但 correct coverage 同时从 24/24 降到 12/24，再次违反 guardrail。它加强了跨模型行为证据，但不构成独立确认、方法全面胜出或跨仓库泛化。

## 1. 问题与方法

匿名化的 governed coding-agent prototype 将完成视为 revision-sensitive state transition，而不是自由文本声明。证据记录绑定 requirement revision 与 artifact digest；修订会使相关证据失效，delivery 只有在证据与当前请求和 artifact 一致时才可 ready。隐藏 research oracle 只在候选构造后做外部评价，不能授权执行或给模型补充事实。

令模型判定为 \(M\)，确定性 binding gate 为 \(G\)：

- **B = M**：模型完成判定；
- **C = M ∧ G**：模型判定与 binding gate 的 conjunction；
- **D = G**：每个唯一候选只计算一次的 gate-only diagnostic。

B 与 C 使用完全相同的模型响应，因此差异来自 gate filtering，而不是额外模型调用或 token budget。

### 研究问题与可证明的边界

RQ1 测量错误完成声明减少多少；RQ2 测量正确交付损失多少；RQ3 区分“当前但错误”和“正确但过期”的失败类型。对于同一模型响应，C = M ∧ G 的接受集合必然是 B = M 的子集，因此 false-ready 不增加本身由结构保证。统计显著不能单独证明方法有用；效应大小、被移除的候选类型，以及预设覆盖率门槛共同决定结论。

生产运行中的实时复审与证据刷新具有额外模型调用和成本，是不同于静态 conjunction 的干预。组件结果不能推导其端到端收益。

## 2. 预注册设计

6 个任务覆盖 pagination、Gregorian leap year、scheduler/reporter ID、HTTP retry parity、settings preservation 与 exponential retry delay。每个任务的 6 个候选中，4 个被外部 oracle 拒绝，2 个接受。`gold_stale_evidence` 与 gold 具有相同正确代码，但 evidence revision/digest stale，用于显式测量 coverage 代价。

主模型 `llama3.2:3b` digest 为 `a80c4f17…`，secondary `llama3.2:1b` digest 为 `baf6a787…`，均由 Ollama 0.4.2 本地提供。主 prompt 为 `revision_rubric_json`，secondary sensitivity 为 `plain_json`。输出必须严格为 `{verdict, reasons}`。temperature=0、top-p=1、context=4096、output limit=32、seeds=17/29。invalid JSON、timeout 或 provider failure 按 ITT 记为 B=C non-ready，不删除、不补跑。

write-once study freeze 在首个 generation 前绑定了 plan、dataset、dependencies、两个 model digests、decoding 参数、candidate hashes 和全部 288 个 schedule cells。四个固定 blocks 依次为 3B/rubric、3B/plain、1B/rubric、1B/plain，每个 72 calls。

唯一 confirmatory test 是 3B/rubric 下 C-vs-B 的六 cluster 双侧 exact sign test，alpha=0.05。primary outcome 是 4 类 externally-rejected candidates 上的 false-ready rate。guardrail 是 `gold` 与 `gold_stale_evidence` 上的 correct coverage，允许最多下降 5pp。三个 secondary comparisons 作为一个 family 做 Holm correction。

### 2.1 DeepSeek model-external follow-up

在本地研究结束后，我们冻结了相同 36 snapshots、prompts、schedule、external labels、estimand、guardrail 与 ITT 规则的 144-call DeepSeek follow-up。`deepseek-v4-pro` 在研究前通过通用 provider qualification 7/7；但 API alias 只表示有日期的服务状态，不是不可变 weight digest，replicate labels 也不是 API seed。

v1 的 128-token 上限导致 26 个 ceiling-linked JSON truncations，attrition 18.1%，所以不报告确认性推断。保留该运行后，我们另写 v2 plan，仅将上限提高到 256，全部请求重新执行，不复用或合并 v1 响应。由于 v1 结果已知，v2 是透明的 operational repair，而不是新的独立预注册。

### 2.2 研究流程中的 AI 辅助

上述 Llama 3.2 模型是被测系统。除此之外，AI research assistants 协助设计 adversarial candidates、实现和审查 harness、执行本地实验以及编辑论文。AI 输出不决定外部标签或报告 observation；标签来自可执行 Rust oracle 与确定性 preservation/integrity checks，统计量从冻结 raw evidence 复算。所有 protocol/code 变更均在 study freeze 前检查，AI 未生成缺失运行、观察结果或引用。

## 3. 结果

### 3.1 完整性与 attrition

两次运行都完成 144/144，`audit.valid=true`，总计 288/288。3B 严格有效输出为 64/144；80 个失败全部是 30 秒 transport timeout。1B 有效输出为 93/144；51 个失败是截断或未闭合 JSON。两个模型的所有严格有效输出均为 `READY`。失败按冻结的 ITT 规则保留为 non-ready。

### 3.2 Primary false-ready

| 指标 | B：模型单独判定 | C：模型 ∧ gate |
| --- | ---: | ---: |
| External false-ready | 23/48（47.9%） | 13/48（27.1%） |
| Externally-correct coverage | 10/24（41.7%） | 5/24（20.8%） |
| Correct delivery / 72 scheduled | 10/72（13.9%） | 5/72（6.9%） |

六个 cluster 的 C−B false-ready differences 为：pagination −12.5pp、century leap year −12.5pp、scheduler/reporter ID −12.5pp、HTTP retry parity −25.0pp、settings preservation −37.5pp、retry delay −25.0pp；wins/losses/ties 为 6/0/0，双侧 exact `p=0.03125`。task-cluster bootstrap 95% interval 为 [−29.2, −14.6]pp。全部 64 个 paired-cluster sign assignments 的预注册 descriptive 95% null-reference interval 为 [−16.7, +16.7]pp；它描述 sign-flip null distribution，不是 observed effect 的 confidence interval。

### 3.3 Guardrail 与机制解释

coverage 下降 20.8pp，超过预设 5pp margin，因此不能声称 C 为无条件方法胜利。C 移除了 baseline 上 5 个和 stale 上 5 个 B claims；但 over-broad 上 7 个、test-tampering 上 6 个 false-ready 均保留，因为其 binding metadata 是 current。同时，C 移除了 `gold_stale_evidence` 上全部 5 个正确 claims。

D 在 36 个唯一候选上有 18 个 ready：externally correct 为 6/12，externally incorrect 为 12/24。它对 correct 与 incorrect 都是 50% coverage，说明 binding gate 只识别 freshness，不能替代语义 oracle。

### 3.4 Secondary analyses

3B/plain 的 aggregate difference 同样为 −20.8pp，但只有 4 wins、2 ties，raw `p=0.125`。1B/rubric 为 −41.7pp（6 wins），1B/plain 为 −37.5pp（5 wins、1 tie）。三个 secondary raw p-values 为 0.03125、0.0625、0.125；Holm-adjusted 为 0.09375、0.125、0.125，均未达到 0.05。1B/rubric coverage 也从 21/24（87.5%）降至 10/24（41.7%）。

### 3.5 DeepSeek operational repair

DeepSeek v2 有 143/144 个有效响应（attrition 0.69%），全部 144 cells 有 terminal event，provider-reported 总用量为 106,691 tokens，artifact 独立重算审计无错误。主条件下 B false-ready 为 30/48，C 为 12/48，paired difference −37.5pp；六个 task differences 为 −25、−25、−50、−50、−50、−25pp，双侧 exact sign test `p=0.03125`，task-cluster bootstrap 95% interval [−45.8, −29.2]pp。

guardrail 仍失败：B 对 externally accepted candidates 的 coverage 为 24/24，C 为 12/24（−50pp）。C 保留全部 12 个 current-bound gold claims，但阻断全部 12 个 behaviorally-correct `gold_stale_evidence` claims。plain secondary 的差为 −39.6pp，方向一致但不能挽救 guardrail。

## 4. 有效性边界

- 这是 completion-decision component study，不是模型生成 patch 的 end-to-end coding benchmark。
- 独立单位只有 6 个 task clusters；0.03125 是六个 cluster 全部同向时可取得的最小双侧 sign-test p 值。
- 3B 仅 44.4%、1B 仅 64.6% 的调用产生严格有效 JSON；高 attrition 是结果的一部分。
- 两个小模型均未通过 coding/high-risk provider qualification，不能代表更大模型或生产 agent。
- DeepSeek 通过通用 qualification，但首轮超过冻结 attrition 门槛；v2 是看到首轮 ceiling failure 后的 outcome-aware repair，并使用可变的 dated API alias。
- 任务来自一个人工 Rust fixture，不能直接外推到其他仓库、语言、provider 或真实用户工作流。
- 预注册和两次独立 pre-call review 降低选择性分析风险，但不能消除 authored task selection 的外部有效性限制。
- DeepSeek v1/v2 不合并 p 值；v2 的上限修复与 v1 失败都必须在投稿中保留。

### 新增端到端证据审计（2026-10-14）

下载检查了仍在运行的 x86_64 工作流中一个任务的四个 cells：全部 session failed、output tokens=0、patch 为空、oracle unknown，尽管该任务 job 显示 success。这里只能判断该样本不合格，不能据此断言全量失败率或 provider 根因。runner 已修复 failed/cancelled 的错误分类，unknown correctness 保留缺失值；工作流下载也改为保留任务目录，避免同名 results 文件互相覆盖。旧冻结记录不改写；任何修复后或 NIM 新模型实验都必须建立新 lock 与 pre-call freeze。

## 5. 结论

revision/artifact binding 在预注册本地 externally-rejected 候选总体中减少了 20.8pp external false-ready risk（`p=0.03125`），DeepSeek operational repair 又观察到 −37.5pp（`p=0.03125`）。前者 coverage 下降 20.8pp，后者下降 50pp，两个 usefulness guardrails 都失败。正确解释是“跨模型一致的描述性 safety gain + 实质 availability cost”，而不是方法全面胜出。下一步必须是 repository-disjoint end-to-end study，并加入 automatic evidence refresh 来测试能否保留 safety benefit、减少正确工作的阻断。

## Data Availability

匿名复现包将作为投稿 supplementary material 随稿提交，包含预注册计划、6-task dataset、外部 Rust oracle、本地 288-call freeze、两套 DeepSeek follow-up freezes、model manifests、append-only events、results、summaries、audits 与复现说明。本地模型用不可变 Ollama digest 标识，远程模型用 provider alias 与运行日期标识。发布前会扫描本地路径、仓库身份、机器标识和凭据；为保持双盲，持久公开 URL 在接收后补充。

该扫描触发了一项 post-run security redaction：runner 单元测试中的 credential-shaped literal 被替换为 synthetic token。pre-call freezes 保留原始哈希，declared-redaction audit 验证 requests、events、results 与 analyses 均未变化。

## AI-Assisted Research Disclosure

AI assistants 参与了 adversarial candidate 设计、harness 实现与审查、本地实验执行和论文编辑。所有 protocol/code 变更都在 study freeze 前经人工或独立 agent 审查；外部标签由可执行 Rust oracle 与确定性 integrity checks 产生，统计量从 raw artifacts 复算。未使用 AI 伪造 observation、citation 或缺失运行。
