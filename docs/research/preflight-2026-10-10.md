# FSE 研究启动与预检记录

日期：2026-10-10。此次完成研究定位、benchmark 审计、实验协议和评分工具；没有得到
可以用于论文效果比较的 live coding 结果。

## 已交付

- [研究定位与相关工作](fse-positioning.md)：推荐研究需求修订后旧证据失效和错误完成声明。
- [实验协议](benchmark-protocol.md)：同模型、同预算、独立 oracle、配对消融与统计计划。
- [现有 benchmark 审计](benchmark-audit.md)：列出测量有效性缺陷和准确的运行命令。
- `scripts/research_metrics.py`：读取已有 AlignmentReport，将外部机械检查、完成声明、
  正确交付和 gate 一致性分别统计；提供按任务类型分层和描述性 Wilson 区间。
- `scripts/test_research_metrics.py`：6 项测试通过；含 gate=ready 但外部失败、零检查、
  无完成声明、非法输入、重复任务和正确交付分母。

评分工具用法（仓库根目录）：

```text
python3 -m unittest scripts/test_research_metrics.py
python3 scripts/research_metrics.py PATH_TO_REAL_ALIGNMENT_REPORT.json --out json
python3 scripts/research_metrics.py REPORT_A.json REPORT_B.json --out target/research/metrics.json
```

脚本不调用模型、不执行候选仓库、不生成实验数据。它的 external correctness 仍然只
反映旧报告中的机械检查；无法弥补 substring oracle、未停止 worker 或用户可见完成
声明未记录等缺口。多份报告不会自动合并，避免把相同任务的重复运行当成独立样本。

## 实际运行结果

| 检查 | 结果 | 边界 |
| --- | --- | --- |
| 当前工作区 CLI 构建 | 通过 | `cargo build -p purrcode-cli --bin purrcode` |
| Rust 格式 | 通过 | `cargo fmt --check` |
| Rust lint | 通过 | `cargo clippy --workspace --all-targets -- -D warnings` |
| Rust 全 workspace tests | 未完成 | 启动首个 agent-runtime 测试进程后约 600 秒没有测试输出，主动停止 |
| evaluation-runtime + benchmark feature tests | 未完成 | 编译阶段等待，主动停止；没有测试 verdict |
| 新增 Python 分析器 | 通过 | 6 tests |
| Qwen2.5-Coder 7B 本地 qualification | 中断 | 加载模型后机器严重变慢，主动停止；没有完整报告，不判模型通过或失败 |
| Qwen2.5 7B qualification | 未运行 | 停止原顺序预检后未启动 |
| Llama3.2 3B qualification | 超时 | 180 秒上限，没有完整报告 |
| LM Studio | 不可用 | 本机 127.0.0.1:1234 连接被拒绝 |
| 新的 live coding benchmark | 未运行 | 尚无此次完整资格报告，且 runner/oracle 有有效性阻塞项 |

两个本地模型测试均调用真实 `purrcode model qualify`，使用独立目录里的 local-only
配置和环境变量白名单，没有读用户 provider 配置或启用远端 fallback。测试之后已停止
本次启动的进程并卸载本次加载的模型；保留 Ollama 服务。启动/编译停滞原因尚未确定，
不能将其直接归因为模型、代码回归或 macOS。内存仅 8 GiB，7B 驻留约 5.6 GB 的观测说明
本机不宜直接承担大规模并发实验；后续须把模型推理和编译串行化，再复核环境。

## 证据

- [机器可读预检记录](evidence/2026-10-10/preflight.json)：HEAD、脏工作区状态、二进制
  SHA-256、模型 digest、资源信息、命令退出状态和运行边界。
- [clippy 原始日志](evidence/2026-10-10/clippy.log)。
- [未完成的 workspace 测试日志](evidence/2026-10-10/workspace-test-incomplete.log)。
- `target/research/20261010T151754Z/manifest.json` 保存本机全部 tracked file hashes 和详细
  原始记录；`target/` 不受版本控制，清理后会丢失。当前已有未提交代码，未归档可重建的
  完整代码快照，因此这不是最终论文 replication package。

本次只新增研究文件和分析脚本，没有修改已有 runtime、工作区改动、release milestone
或历史资格报告。新的证据保存在此处，避免覆写之前的真实失败与部分成功记录。

## 下一阶段验收顺序

1. 排查 Rust 测试/编译启动停滞；在推理停止时恢复完整验证。
2. 修复研究 runner：唯一目录、HTTP 检查与超时、停止并等待 worker、不可变最终快照、
   有上限的独立验证、逐 run 持久化，区分内部状态与用户可见完成声明。
3. 做六个有 reviewed behavioral oracle 的新 pilot 任务，再接入 A/B/C 三个对照。
4. 用完整资格化模型执行 36 次 pilot；这仍是未来安排，未执行。
5. 用 pilot 方差和成本决定正式样本量，冻结数据与分析后再跑主实验。

优先完成测量有效性和 pilot，再决定是否值得投入约千次运行。不会用本次预检或已有
20 个开发 fixture 宣称达到 FSE 论文证据标准。
