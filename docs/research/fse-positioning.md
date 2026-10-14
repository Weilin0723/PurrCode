# FSE 研究定位：可审计的修订感知完成判定

日期：2026-10-10  
状态：研究定位与 benchmark 设计；没有把未运行的 live provider、实验或结果写成证据。

## 先说投稿窗口

FSE 2027 Research Track 的官方 CFP 记录了 HotCRP 故障后的延期：全文截止日为
2026-10-03 AoE；官方日期页仍列出原定的 2026-10-02。当前日期是 2026-10-10，
所以这份工作不能再作为 FSE 2027 Research Track 的新投稿。FSE 2027 会议时间是
2027-07-12—16，官方 CFP 尚未给出下一届 FSE 的日期；这里把下一轮（暂称 FSE 2028）
作为目标，不猜测未发布的 deadline。

FSE Research Track 接受经验、概念和实验软件工程研究，主题明确覆盖 AI/ML for SE、
requirements engineering、dependability/safety/reliability、software security、testing
和 tools/environments。投稿限制是正文与图不超过 18 页、参考文献不超过 4 页；FSE 2027
采用严格双匿名审稿，并要求在匿名条件下说明 replication package 的可用性。

来源：

- [FSE 2027 Research Papers CFP](https://conf.researchr.org/track/fse-2027/fse-2027-papers)
- [FSE 2027 official dates](https://conf.researchr.org/dates/fse-2027)
- [FSE 2027 home page](https://conf.researchr.org/home/fse-2027)

## 仓库能支持的研究主张

PurrCode 已经有一个适合做实证研究的控制面：PawGate 做确定性判断，NineLives 持久化
授权与事件，Claw 再次核对 action/constraint digest 后执行，工作区使用隔离 worktree。
v1.5 又增加了引用用户原话的 expectation contract、可解析的 evidence、独立 fresh-context
review、bounded correction 和由 durable log 重新计算的 delivery gate。这些语义与
`docs/architecture.md`、`docs/v1.5-architecture.md`、`docs/security.md` 和
`docs/production-acceptance.md` 一致。

当前证据的边界同样清楚：

- `evaluation-runtime` 的 20 题是 5 个 simple、10 个 complex、5 个 alignment trap，已经
  用于实现和回归，不能作为独立的 confirmatory test set。
- 这些题的 `Contains`/`Lacks`/`Exists` 检查是代理标签，可能接受注释或非功能代码；仓库
  测试也可能被被测 agent 修改，不能单独当作语义正确性 oracle。
- `docs/reports/benchmark.md` 记录 v1.5 live 目录因远端 provider HTTP 402 而中断；当前本机
  7B local qualification 因 8 GB 机器变慢而中断，只有环境预检证据，不能写成通过或失败，
  也不能作为已资格化的本地模型。`docs/reports/recovery-validation.md` 通过了事件模式的恢复
  测试，但没有把真实 provider 下的执行中崩溃写成已完成实验。

因此论文的对象应是“修订感知、证据绑定的完成判定机制及其外部评估”，不是 IDE 功能
数量、提示词集合或一次 live 分数。

## 相关工作与可用的差异点

| 工作 | 已经测量的东西 | 对 PurrCode 研究的约束 |
| --- | --- | --- |
| [SlopCodeBench](https://arxiv.org/abs/2603.24755) | 在迭代规格和多个 checkpoint 上测量 structural erosion 和 verbosity，并报告跨 checkpoint 的退化。 | 不能声称“首次研究多轮 coding”或“首次研究规格变化”。PurrCode 要测的是用户修订后旧 evidence 是否失效、系统是否错误地完成，不是代码侵蚀。 |
| [SWE-EVO](https://arxiv.org/abs/2512.18470) | 48 个真实软件演化任务，平均跨 21 个文件，评估长程、多文件版本演化。 | 这是能力/演化基准；PurrCode 的区分点必须是 control-plane invariant、revision binding 和 completion decision，并且应报告在相同 agent 上的代价。 |
| [Are “Solved Issues” in SWE-bench Really Solved Correctly?](https://software-lab.org/publications/icse2026_SWE-bench-correctness.pdf)（[ICSE 2026 页面](https://conf.researchr.org/details/icse-2026/icse-2026-research-track/59/Are-Solved-Issues-in-SWE-bench-Really-Solved-Correctly-An-Empirical-Study)） | PatchDiff 发现 test-passing patch 仍可能和 developer patch 行为不同；论文报告 7.8% 的 plausible patches 未通过全部 developer tests，29.6% 与 oracle 行为可区分。 | 现有字符串检查不够。至少要有隐藏行为测试、保持性检查，必要时用差分测试或人工审计区分“gate 一致”与“外部正确”。 |
| [SWE-Bench Pro Verified](https://arxiv.org/abs/2609.08149) | 通过去除 gold-solution/隐藏评估泄漏和修正任务质量问题，说明 benchmark reward hacking 会夸大 agent 能力。 | 要锁定 source commit、去除 gold/hidden test 泄漏、限制网络，并把任务质量和 oracle 审计当成数据集工作，而非事后解释。 |
| [τ-bench](https://openreview.net/pdf?id=roNSXZpUDN) | 多轮 user-agent-tool 对话、领域 policy、状态化 API 和 `pass^k` 一致性。 | 可借用重复试验和最终状态 oracle；PurrCode 需要把状态化目标换成代码树、requirement revision、evidence 与 delivery gate。 |
| [AgentDojo](https://proceedings.neurips.cc/paper_files/paper/2024/file/97091a5177d8dc64b1da8bf3e1f6fb54-Paper-Datasets_and_Benchmarks_Track.pdf) / [InjecAgent](https://aclanthology.org/2024.findings-acl.624/) | 在不可信工具返回内容中测试 prompt injection、恶意工具调用和安全属性。 | 这支持把恶意 repository text 作为安全压力项，但不应把 PurrCode 论文扩大成通用 prompt-injection benchmark。 |

## 三个可检验方向

### 方向 A：用户修订后的证据绑定完成判定（推荐）

候选标题：**When Requirements Change: Evidence-Gated Completion for Coding Agents**。

核心问题是：用户在 agent 已经开始工作后修订要求时，系统能否让旧计划和旧 evidence
失效，并阻止“已经完成”的错误声明？贡献应是一个可重放的机制：每条 requirement、
evidence、review finding 和 artifact state 都绑定当前 contract revision；correction
会使受影响 evidence 回到 unverified；只有针对当前 revision 的 runtime-visible validation、
fresh review 和 durable reducer 才能触发 ready。hidden
behavioral oracle 不进入 agent context 或 runtime gate，只在 run 停止并完成快照后做独立
external evaluation。

可检验的 RQ：

1. 在相同模型、工具、任务和 token ceiling 下，revision-bound completion 是否降低
   “用户已修订但仍按旧要求完成”的 stale-completion rate？
2. 相对于 checklist-only 和 review-without-revision-binding，full method 是否降低
   external false completion，同时保持 correct delivery 不低于预先声明的 5 个百分点非劣
   效应界线？
3. 错误率降低来自 revision invalidation，还是只是更多 review/repair token？
4. correction acknowledgement、revision reflected、external correctness 和
   delivery gate 是否在 daemon restart 后保持一致？

最小对照：

- A：同一 coding loop，只给最新请求和 checklist；没有 durable completion gate。
- B：同一 loop 加独立 review/bounded repair，但不绑定 revision evidence。
- C：PurrCode full method。
- D：C 的研究性 stale-evidence ablation，correction 后保留受影响 evidence。

所有 arm 必须保留相同 PawGate、精确授权、Claw digest recheck、credential scrubbing
和 worktree isolation；研究 ablation 不得在生产执行路径上绕过授权。

主要指标：外部 oracle 判定的 stale completion、external false completion、correct delivery、
correction reflected rate、requirement satisfaction、blocking finding repair rate、
scope escape、review overhead、token/latency 和 restart 后的 evidence continuity。
主分析应是配对的 C 对 B；重复 seed 不能当成独立任务，按 base task/repository 聚类
给区间。

这是最适合当前代码的方向，因为 v1.5 已经有 contract/review/correction/gate 的可重放
结构，但“选择性使 evidence 失效”和各 arm 的公平接入仍需审计和实现，不能直接假设已
完成。它也能把现有 20 题作为开发集，把真正的研究问题放到新任务和新 correction 上。

### 方向 B：编码 agent 的授权—执行一致性

问题是：typed action、durable single-use authorization 和执行前 digest recheck 是否
能在不明显损失任务效用的情况下阻止 replay、argument substitution、越权写入和来自
不可信 repository text 的危险 action。

实验可在隔离 fixture 中构造 benign coding tasks、恶意文件内容、改写 action 参数、并发
消费同一 authorization 和执行中断。对照是 prompt-only policy、仅一次 approval 但无
执行端 digest recheck、以及 full PawGate→NineLives→Claw。指标是 unauthorized execution
rate、duplicate effect rate、benign task success、拒绝的 false positive、额外 latency
和 audit completeness。AgentDojo/InjecAgent 是攻击面和报告术语的参考，不是直接可比的
代码 benchmark。

该方向的优点是安全不变量清楚且有 mutation tests；风险是容易变成 runtime/security
工程报告，和已有 agent security benchmark 重叠，也难以证明真实 coding utility。若没有
足够多的可复现攻击和跨 host sandbox 证据，不宜作为主论文。

### 方向 C：中断后的证据与副作用恢复

问题是：在 authorization consume、tool execution、validation、review 或 correction
边界发生崩溃后，durable event log、uncertain 状态和 checkpoint 是否能阻止重复执行、
丢失 correction 或把未验证状态报告成完成。

实验用故障注入而不是等待真实随机崩溃：在预先注册的事件边界杀掉 daemon，然后分别用
cold restart、checkpoint-only 和 full NineLives recovery 重启。指标是 duplicate action、
lost evidence、stale contract、false completion、recovery time、unresolved/uncertain
率和人工介入次数。PurrCode 已有事件 reducer 和 restart simulation 的机械证据，但真实
provider 执行中崩溃、跨平台效果和外部副作用仍是风险。

这个方向可作为 A 的 robustness factor；单独投稿时新颖性和同类恢复基准的边界需要更
全面检索，且恢复语义不能只由同一个 runtime 自己打分。

## Benchmark 安排与停止条件

1. **先冻结研究设计。** 现有 20 题只用于 harness regression。新增任务按 repository-
   disjoint 的 development/pilot/held-out 切分；每题保存 source commit、用户原话、
   correction、gold patch、行为 oracle、preservation oracle、任务许可证和 task hash。
2. **先做 oracle，再跑模型。** 用独立的 hidden behavioral tests、不可由 agent 修改的
   preservation checks、negative/stale/over-broad/test-tampering patches 验证 external
   oracle；这些检查只能在 run 停止并完成快照后执行，不能注入 agent context 或用来让
   runtime gate 提前判定 ready。明确接受替代正确实现，不把 gold diff 当唯一答案。PurrCode
   当前的字符串检查只能标成 proxy。
3. **pilot。** 六个新任务覆盖 scope preservation、changed behavior、API compatibility、
   validation 后 correction、重复 correction 和 benign control；只运行 A/B/C，使用同一个
   已资格化模型做两次配对重复，共 36 runs。D 不进入 pilot。pilot 只用于估计方差、运行
   时间和失败模式，不能改写预注册的主指标。
4. **confirmatory。** 冻结任务、arm、模型、seed、预算和统计计划后，再运行至少六个真实
   repository cluster 的 held-out correction/static pairs，并把 D 纳入主实验。候选规模为
   30 个 base tasks × static/correction 两种变体 × 4 arms × 2 model families × 3 repeats，
   即 1,440 runs；最终 N 应由 pilot 方差、power/precision 和可用算力冻结。报告 scheduled
   denominator、provider qualification、timeout、infra failure、missing usage 和
   unavailable，不能只保留成功轨迹。
5. **本地模型门槛。** 本机 Qwen 7B 的 qualification 当前未完成；3B route 的 180 秒尝试
   超时且没有 qualification report。两者都只能作为 unavailable/incomplete evidence，
   不能重写成通过或失败。只有 local route 通过完整 qualification 才能进主实验；否则把
   本地运行当 pilot/debug。preflight 的阻塞项与后续处理记录在 [benchmark audit](benchmark-audit.md)
   和 [benchmark protocol](benchmark-protocol.md)；使用已资格化 provider 时仍需记录余额、
   网络和版本。凭据不能进入 trace、config 或 child process。
6. **停止/发布条件。** 若 oracle 不能区分 stale、correct 和 over-broad patch，或 C 对 B
   的区间无法判断错误率与正确交付的权衡，就报告 inconclusive；不以扩大运行次数、替换
   失败运行或选择最好 seed 代替证据。

## 主要威胁与审稿人会问的问题

- **重叠风险。** SlopCodeBench/SWE-EVO 已覆盖迭代和长程演化；ICSE 2026 PatchDiff 已
  讨论测试通过但行为错误；SWE-Bench Pro Verified 已讨论泄漏和任务质量。论文必须把主张
  限定为 revision-bound evidence、completion decision 和可重放 correction protocol，
  并在实验中加入这些工作的对照或明确不可比处。
- **oracle 风险。** 现有 fixture 的硬字符串检查会被注释、测试篡改和等价实现混淆。hidden
  行为测试、完整测试、保留性检查、差分测试和独立人工抽样必须分开报告，并与 runtime
  gate 的可见 evidence 分开。
- **成本混淆。** full method 可能花更多 review/repair token；共享 token ceiling、相同
  deadline、同一模型与 tool surface，并报告 cost-response curve。
- **触发偏差。** correction 不能按 PurrCode 是否已经写入 contract 来触发，应按第一个
  成功编辑或第一次 validation 等 scaffold-neutral milestone 触发；未达到 milestone 的
  run 要单独计入 scheduled denominator。
- **外部有效性。** 小型 Rust fixture 只能验证机制；confirmatory set 至少跨多个真实
  repository，并按 repository cluster 给不确定性。
- **安全边界。** 只能声称已测试 host 上的授权、digest、凭据 scrub 和 sandbox 行为；
  fallback isolation、未测试的跨平台和真实外部副作用都必须列为 external gate。
- **匿名与 AI disclosure。** FSE 2027 CFP 要求双匿名、replication package 说明和对直接
  影响研究结论的 AI 使用作方法披露；未来投稿需在提交版本隐藏作者/组织/自指链接，并
  逐条核验引用，不能让模型生成未经核验的结果、数据或引用。

## 建议的主线

选择方向 A，把方向 B 的授权一致性作为安全边界检查，把方向 C 的故障注入作为恢复压力
轴；不要在一篇论文里同时声称三个独立新系统。论文的可证伪主张应是：**在任务要求发生
用户修订时，将 completion evidence 绑定到当前 revision，并由 runtime-visible validation、
review 和 durable gate 决定交付；run 停止后的 hidden oracle 再独立测量 stale/external
false completion。该机制应在预先声明的正确交付和成本界线内降低错误完成。**

如果 pilot 不能先证明外部 oracle 可靠，应该把结果定位为 benchmark/protocol paper 或
暂缓 FSE，而不是用现有 20 题的字符串通过率支撑系统性结论。
