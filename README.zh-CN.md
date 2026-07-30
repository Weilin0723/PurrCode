# PurrCode

[English](README.md) · [文档](docs/) · [最新版本](https://github.com/Weilin0723/PurrCode/releases/latest)

[![CI](https://github.com/Weilin0723/PurrCode/actions/workflows/ci.yml/badge.svg)](https://github.com/Weilin0723/PurrCode/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/Weilin0723/PurrCode)](https://github.com/Weilin0723/PurrCode/releases/latest)
[![License](https://img.shields.io/github/license/Weilin0723/PurrCode)](LICENSE)

**一个本地优先、拥有独立且可审计判断运行时的编程智能体。**

> 模型负责提议，PawGate 负责授权，Claw 负责执行，证据决定结果。

PurrCode 是一个在隔离 Git worktree 中工作的终端编程智能体。每个原生操作都必须绑定到
持久化授权，并在执行前再次校验，随后记录真实验证结果。仓库内容、模型输出和下载的技能
始终被视为不可信数据。

PurrCode v0.8.0 新增安全的图形化 Studio、持久化 Workbench、真实 PTY 终端工作区、
基于证据的环境诊断，以及自动渐进式构建／测试修复。PawGate、Claw、隔离 worktree 与
持久化证据边界仍然是唯一可信执行路径。

## 为什么选择 PurrCode

- **可强制执行的授权：** PawGate 对序列化后的具体操作和约束进行审批；执行适配器会再次
  校验，调用方无法绕过。
- **保护现有工作区：** 智能体的修改保留在隔离 worktree 中，只有经过审查才会应用。现有
  未提交内容不会被静默 stash、覆盖或删除。
- **以证据判断完成状态：** 通过、失败、超时、不可用和跳过是不同状态；跳过的验证绝不会
  被显示为成功。
- **保守的故障恢复：** NineLives 从持久化事件恢复会话，并将中断后的不确定副作用标记为
  需要审查，而不是盲目重放。
- **模型供应商没有授权能力：** 支持 Ollama、LM Studio、OpenAI 兼容接口、企业网关和
  Codex bridge，但模型不能批准自己的操作。
- **受治理的技能和研究：** 技能需要经过检查、资格验证和逐次授权；公开网络访问也必须遵守
  明确策略。

## 安装

### npm

```bash
npm install --global @minaovo/purrcode
```

使用 Node.js 18 或更高版本，也可以直接从 GitHub 安装同一个已签名 launcher：

```bash
npm install --global https://github.com/Weilin0723/PurrCode/releases/download/v0.8.1/purrcode-0.8.1.tgz
```

安装包会选择正确的 macOS、Linux 或 Windows 二进制文件，校验固定的 SHA-256 摘要，并
提供 `purrcode` 和 `purrcoded` 两个命令。

### macOS 和 Linux 安装脚本

```bash
curl -fsSL https://raw.githubusercontent.com/Weilin0723/PurrCode/v0.8.1/scripts/install.sh | sh
```

脚本会使用 `SHA256SUMS` 校验发布归档，并默认安装到 `~/.local/bin`。可通过
`PURRCODE_INSTALL_DIR` 指定其他目录。

### 从源码构建

需要 Rust 1.88 或更高版本：

```bash
cargo install --locked --path crates/purrcode-cli
cargo install --locked --path crates/purrcode-daemon
```

## 三步开始使用

```bash
# 1. 发现本地模型供应商并创建安全默认配置
purrcode init

# 2. 进入项目仓库
cd your-project

# 3. 打开图形化 Studio（或使用 `purrcode tui`）
purrcode ui
```

在界面内使用 `/connect` 即可发现 Ollama 或 LM Studio，也可以配置远程供应商，无需手动
编辑 TOML。凭据存储在操作系统密钥库中，不会进入模型上下文或工具进程。

使用 `/connect import` 可粘贴 Python、JavaScript、cURL、JSON、YAML、TOML 或 dotenv
示例。PurrCode 只解析、不执行；提取出的密钥只短暂保存在内存中，保存前必须转换为钥匙串或
环境变量引用。

## v0.8 更新

- **安全的图形化 Studio：** `purrcode ui` 启动仅绑定 loopback 的认证应用；daemon 凭据
  始终保留在服务端，用户提交任务之前不会触发模型生成。
- **持久化工程 Workbench：** 对话、活动、diff、验证和精确证据均从 daemon 持久状态恢复，
  且不会展示隐藏的模型推理。
- **真实终端工作区：** 原生 PTY／ConPTY 会话支持标签、有限转录重放、缩放、detach、停止，
  以及带 generation 校验的人机控制权切换。
- **基于证据的环境诊断：** 有界仓库／主机检查识别所需工具链并记录真实版本探测；缺失工具
  会明确报告，绝不会冒充成功。
- **自动测试修复：** 测试编排器识别主流构建系统，持久化精确授权的验证操作，分类失败并
  路由有限修复；只有全部必要的最终证据通过后才允许完成。

## v0.7 更新

- **可复现的安全评估：** 版本化 benchmark 通过生产 PawGate 与 Claw 路径运行，校验预期
  拦截操作和禁止副作用，并报告 Safe Autonomy Rate。
- **可验证证据：** trace 与 explain 命令展示持久化决策；原子写入、脱敏的证据包可以在
  不执行副作用的情况下离线检查、验证和重放。
- **真实的恢复测试：** 对持久化、索引、影响收集和导出注入失败，确保中断或不确定状态
  不会被报告为成功。
- **更安全的 Provider 接入：** OpenAI 兼容请求示例只解析、不执行；密钥不会进入保存的
  配置；手动设置会分别要求 Base URL、认证引用和 Model ID。
- **资源感知的模型切换：** `/models` 提供可交互选择器并持久化选择；当内存或资格证据不
  支持当前模型时，会明确建议更小的模型。
- **一致且可复制的 TUI：** 明暗主题背景仅使用纯白或纯黑，语义颜色保持一致；失败后的
  模型输出不会消失，并支持 macOS Terminal 原生拖选复制。

## v0.6 更新

- **类型化仓库读取：** 文件、目录、搜索和只读 Git 操作从模型 schema 到 PawGate、Claw
  始终使用结构化操作；不安全路径和含糊的旧命令会在授权前安全拒绝。
- **确定性会话状态：** 单一 reducer 校验所有生命周期转换，并拒绝过期或不匹配的审批，
  不会破坏当前会话。
- **可靠的完成输出：** 纯建议任务会将具体编号计划保存为持久输出；执行任务只依据已验证
  的工具结果继续，并保留真实失败状态。
- **可用的长对话：** 助手文本按时间线宽度换行，时间线支持键盘和鼠标滚动并自动跟随最新
  活动，卡片可通过点击、Space 或 E 展开。
- **干净的流式重试：** 重试会替换不完整尝试，不再重复拼接推理文本，也不会停留在过期
  的流式状态。

## v0.5 更新

- **可靠的 Provider 路由：** 已保存的钥匙串凭据、远程 Provider 路由、低内存 Ollama
  默认值和 NVIDIA NIM 有界生成探测都会使用正确的配置与模型。
- **明确的会话恢复：** 启动时会先要求选择继续会话、只读查看历史或新建会话；已经结束的
  会话不会被静默重放。
- **审批后自动续跑：** 持久化审批边界会等待上一个 daemon lease 释放；无效审批不会破坏
  会话状态；已批准的操作执行后，智能体会自动继续下一步。
- **更清晰的终端工作流：** 默认使用纯黑背景和高对比度文字，始终显示当前 Provider/模型；
  Space 或 E 可展开时间线详情，终端选择内容仍可复制。
- **真实流式体验：** 内容增量、Provider 阶段和持久化审计使用三个独立的有界通道；重连
  恢复快照，取消时保留 partial 输出。
- **资源感知的本地模型：** 启动时不会生成内容或加载模型。Ollama 默认使用原生接口；推荐
  只依据已观察的资格证据和内存状态；低内存设备默认单请求并在结束后卸载。
- **受治理的能力发现：** 优先复用已安装且合格的 Skill。公开搜索、固定提交下载、动态资格
  验证、安装和每次 MCP 调用都需要独立的精确 PawGate 授权。
- **延迟上下文：** 启动时 Tier 0 只读取元数据；提交任务后才运行 Tier 1；有界 Tier 2 会在
  生成、内存压力或响应变慢时暂停。

常用界面命令：

```text
/connect import
/model recommend
/model qualify <model>
/model loaded
/model unload <model>
/skills search <query>
/mcp search <query>
/capability add <description>
```

## 运行时模型

```text
模型提议
  → PawGate 策略与独立判断
  → 持久化的精确操作授权
  → Claw 再次校验并隔离执行
  → 验证证据
  → 审查后应用或回滚
```

| 组件 | 职责 |
|---|---|
| **PawGate** | 确定性策略、语义审查、约束和人工审批关卡 |
| **Claw** | 在 worktree 范围的操作系统沙箱中执行，并清理凭据环境 |
| **Whisker** | 有界上下文检索、敏感文件过滤和风险信号 |
| **NineLives** | 持久化事件、检查点、重启协调和回滚 |

## 支持的界面

- 以对话为中心的 Ratatui 终端和无界面 CLI
- 使用服务端事件的认证回环 daemon
- VS Code 扩展
- TypeScript 和 Python 客户端
- MCP 与持久化技能主机
- Ollama、LM Studio、OpenAI 兼容接口和企业供应商

## 常用命令

```bash
purrcode plan "为订单 API 添加分页"
purrcode run "实现分页并更新测试"
purrcode sessions
purrcode review
purrcode approve
purrcode resume
purrcode rollback
```

## 安全与验证

PurrCode 在 macOS 上使用 `sandbox-exec`，在受支持的 Linux 主机上使用 Bubblewrap。
较弱的主机隔离能力会被如实标记，绝不会伪装成完整沙箱。在敏感仓库中使用前，请阅读
[安全模型](docs/security.md)、[架构说明](docs/architecture.md)和
[生产验收审计](docs/production-acceptance.md)。

仓库验证命令：

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
npm test --prefix packages/purrcode
npm test --prefix sdk/typescript
npm test --prefix apps/vscode-extension
PYTHONPATH=sdk/python/src python3 -m unittest discover -s sdk/python/tests -v
```

## 文档

- [安装](docs/installation.md)
- [供应商配置](docs/providers.md)
- [架构](docs/architecture.md)
- [安全](docs/security.md)
- [恢复](docs/recovery.md)
- [故障排查](docs/troubleshooting.md)
- [实现状态](docs/implementation-status.md)
- [v0.5 可用性恢复证据](docs/reports/v0.5-usability-recovery.md)
- [v0.5.2 Provider 路由修复证据](docs/reports/v0.5.2-provider-routing-hotfix.md)
- [v0.4 重构验收](docs/product-redesign-acceptance.md)
- [验证报告](docs/reports/)

PurrCode 仍在积极开发中。请查看实现状态和发布说明，确认已经验证的能力以及仍待完成的
平台专项验收。

## 许可证

Apache-2.0，详见 [LICENSE](LICENSE)。
