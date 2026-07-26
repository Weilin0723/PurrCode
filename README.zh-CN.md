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

### npm 兼容安装包

使用 Node.js 18 或更高版本，可以直接从 GitHub 安装：

```bash
npm install --global https://github.com/Weilin0723/PurrCode/releases/download/v0.4.0/purrcode-0.4.0.tgz
```

安装包会选择正确的 macOS、Linux 或 Windows 二进制文件，校验固定的 SHA-256 摘要，并
提供 `purrcode` 和 `purrcoded` 两个命令。

### macOS 和 Linux 安装脚本

```bash
curl -fsSL https://raw.githubusercontent.com/Weilin0723/PurrCode/v0.4.0/scripts/install.sh | sh
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

# 3. 打开以对话为中心的终端界面
purrcode
```

在界面内使用 `/connect` 即可发现 Ollama 或 LM Studio，也可以配置远程供应商，无需手动
编辑 TOML。凭据存储在操作系统密钥库中，不会进入模型上下文或工具进程。

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

PurrCode 仍在积极开发中。请查看实现状态和发布说明，确认已经验证的能力以及仍待完成的
平台专项验收。

## 许可证

Apache-2.0，详见 [LICENSE](LICENSE)。
