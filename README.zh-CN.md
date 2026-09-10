<p align="center">
  <img src="assets/banner.svg" alt="whycodes — 面向终端的快速、与供应商无关的编程智能体，使用 Rust 编写" width="720">
</p>

<p align="center">
  <a href="https://olud.ai/tool/whycodes.html"><img src="https://olud.ai/badge.php?tool=whycodes" alt="olud.ai"></a>
  <a href="https://github.com/whycorporation/whycodes/actions/workflows/ci.yml"><img src="https://github.com/whycorporation/whycodes/actions/workflows/ci.yml/badge.svg" alt="CI"></a>
  <a href="https://github.com/whycorporation/whycodes/releases"><img src="https://img.shields.io/github/v/release/whycorporation/whycodes" alt="Release"></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-MIT-blue.svg" alt="License: MIT"></a>
  <a href="https://x.com/whycodesai"><img src="https://img.shields.io/badge/X-@whycodesai-black" alt="在 X 上关注 @whycodesai"></a>
  <a href="https://github.com/sponsors/whycorporation"><img src="https://img.shields.io/badge/sponsor-whycorporation-ea4aaa" alt="赞助 WhyCodes"></a>
</p>

<p align="center">
  <a href="README.md">English</a> · 简体中文
</p>

**面向终端的快速、与供应商无关的编程智能体，使用 Rust 编写。**

WhyCodes 可读写并编辑文件、运行命令、搜索工作区，并通过多轮工具调用驱动
大语言模型 —— 既有全屏 TUI，也可作为机器可读的一次性 CLI。它强调较小的
原生体积、空闲时高效的 TUI，以及不绑定任何单一模型供应商的工作流。

<p align="center">
  <img src="assets/tui-home.svg" alt="whycodes TUI 主屏幕" width="820">
</p>

## 亮点

- **单一原生二进制。** 无需运行时依赖。搜索在进程内完成（不需要
  `ripgrep`）；可选的操作系统集成（如 Linux bubblewrap）在可用时加强沙箱。
- **跨平台。** Linux、macOS 与 Windows 使用同一套工作流。每次 CI 变更都会
  跑 Linux；Windows 与 macOS 任务通过多操作系统 CI 矩阵提供。
- **任意供应商。** Anthropic、OpenAI、Google、GitHub Copilot、Groq、xAI、
  DeepSeek、Ollama、OpenRouter、Mistral、Together，或任何 OpenAI 兼容端点
  —— 使用 API 密钥或订阅登录（`whycodes auth login`）。
- **贴合你的项目。** 从 `AGENTS.md` 加载项目说明，并通过 LSP 连接已有的
  MCP 工具与语言服务器。
- **记住你的工作。** 持久化会话、跨会话记忆（`MEMORY.md`、语义召回）以及
  可选的代码索引。

## 安装

### 安装脚本（Linux、macOS）

```bash
curl -fsSL https://why.codes/install | bash
```

### 安装脚本（Windows）

```powershell
irm https://why.codes/install.ps1 | iex
```

会将 `%LOCALAPPDATA%\Programs\whycodes` 加入用户 PATH。不需要 WSL。

### Homebrew（macOS；Linuxbrew x86_64）

```bash
brew tap whycorporation/whycodes https://github.com/whycorporation/whycodes
brew install whycorporation/whycodes/whycodes
```

Homebrew 6+ 会拒绝未受信任的第三方 tap。使用完全限定的安装命令只信任本
formula。若已经 tap 并看到该错误：

```bash
brew trust --formula whycorporation/whycodes/whycodes
brew install whycodes
```

### 从源码构建

```bash
# 需要 libsqlite3（pkg-config）。完全静态二进制：
# cargo build --release -p whycodes-cli --features bundled-sqlite
cargo build --release -p whycodes-cli
```

使用 `whycodes upgrade` 更新（脚本 / cargo 安装），或 `brew upgrade
whycodes`（Homebrew）。交互式 TUI 会话会检查 GitHub 上的新版本，并在主
屏幕安装前提示。传入 `--no-auto-update` 可跳过提示。安装脚本会对照已发布
的 `SHA256SUMS` 校验发行产物。可下载二进制与卸载说明见
[docs/packaging.md](docs/packaging.md)。

<details>
<summary>Shell 补全</summary>

```bash
eval "$(whycodes completions zsh)"    # ~/.zshrc
eval "$(whycodes completions bash)"   # ~/.bashrc
whycodes completions fish > ~/.config/fish/completions/whycodes.fish
```

</details>

## 快速开始

```bash
export ANTHROPIC_API_KEY="sk-ant-..."

whycodes -d ./my-project                            # 交互式 TUI
whycodes generate "Explain main.rs" -d ./my-project # 一次性命令
whycodes generate "Summarize the last commit" --format json
whycodes --continue                                 # 恢复上次会话
whycodes -P openai -m gpt-4o generate "Refactor this module"
```

订阅 OAuth 登录（`whycodes auth login <provider>`）仅在安装本地
`kind: "auth"` 插件后可用 —— WhyCodes 不附带第三方 OAuth 客户端。见
[docs/auth.md](docs/auth.md)。

完整指南 —— CLI 参考、TUI 快捷键、斜杠命令、智能体、工具与配置 —— 见
**[docs/guide.md](docs/guide.md)**。

## 功能

| | |
|---|---|
| **智能体** | 主智能体：`build`（完整权限）、`plan` 与 `ask`（只读）；通过 `task` 工具使用 `general`、`explore`、`scout` 子智能体；通过 `swarm` 在 git worktree 中并行工作 |
| **工具** | 文件编辑/补丁、进程内搜索、shell、git 与 GitHub、网页抓取/搜索、CDP 驱动的浏览器、后台任务、调度与待办跟踪 |
| **会话** | 按项目持久化；用 `--continue` / `--resume` 恢复，从其他智能体 CLI 导入记录，通过本地服务器分享 |
| **记忆** | 可人工编辑的 `MEMORY.md`、带嵌入的语义事实、可选代码 RAG 索引 —— 均按项目、均可选 |
| **无界面 / CI** | `generate` 支持 `--format json` 或 `stream-json`（NDJSON），多个提示可并发运行，失败时非零退出 |
| **安全** | 按工具的权限门、shell 命令风险分析、可选操作系统沙箱（bubblewrap）、HTTP 域名白名单与工具钩子 |
| **扩展** | MCP 服务器（stdio 与 HTTP）、LSP 语言服务器、技能、shell 插件、自定义斜杠命令、主题 |
| **嵌入** | 基于守护进程协议 v1（`whycodes serve`）的 Rust 与 TypeScript SDK |

## 性能

空闲 TUI，Linux x86_64，2026-09-02（方法与机器见
[docs/benchmarks.md](docs/benchmarks.md)）：

| 指标 | 结果 |
|---|---|
| 1 个会话 PSS | **10.5 MB** |
| 10 个会话 PSS | **32.0 MB**（每多一个约 2.4 MB） |
| `--version` | **1.4 ms** |
| 首帧（harness，进程内） | **12 ms** |
| 空闲重绘（harness，3 s） | **0.3 /s** |

TUI 仅在有变化时绘制。本次 3 秒 harness 空闲为 **0.3 次重绘/秒**（与
`ea098af` / `50e05d8` 相同；idle-zero 门控并未恢复硬零）；产品目标仍是
**0 次重绘/秒**，而不是帧率竞赛。

工作区行覆盖率为 **85.58%**（Linux x86_64，2026-08-21）。CI 在低于 82%
时失败，十二个基础 crate 的生产代码行覆盖率保持 100% —— 见
[docs/coverage.md](docs/coverage.md)。

## 文档

| 文档 | 内容 |
|---|---|
| [docs/guide.md](docs/guide.md) | 用法：CLI、TUI、智能体、工具、配置、SDK |
| [docs/auth.md](docs/auth.md) | API 密钥、OAuth、凭据导入 |
| [docs/architecture.md](docs/architecture.md) | Crate 地图与分层 |
| [docs/roadmap.md](docs/roadmap.md) | 当前重点与延后工作 |
| [docs/knowhow.md](docs/knowhow.md) | 来之不易的缺陷（TUI、tty、静默退出） |
| [docs/tui-term-matrix.md](docs/tui-term-matrix.md) | 在 Alacritty / Kitty / VTE 上的手动 TUI 检查 |
| [docs/benchmarks.md](docs/benchmarks.md) | 测量启动、RSS、空闲绘制 |
| [docs/coverage.md](docs/coverage.md) | 测量行覆盖率 |
| [docs/budgets.md](docs/budgets.md) | CI 质量预算 |
| [docs/packaging.md](docs/packaging.md) | Homebrew、安装器、发行下载次数 |

## 贡献

欢迎贡献。[CONTRIBUTING.md](CONTRIBUTING.md) 是从克隆到合并变更的短路径；
[AGENTS.md](AGENTS.md) 约束在本仓库中工作的编程智能体。参与即表示同意
[行为准则](CODE_OF_CONDUCT.md)。帮助与缺陷报告：[SUPPORT.md](SUPPORT.md)。
请通过 [SECURITY.md](SECURITY.md) 报告漏洞，不要开公开 issue。

## 赞助

WhyCodes 独立开发，需要资金才能持续发布。请通过
[GitHub Sponsors](https://github.com/sponsors/whycorporation) 赞助本项目。

## Star 历史

<p align="center">
  <a href="https://www.star-history.com/?repos=whycorporation%2Fwhycodes&type=date&legend=top-left">
    <picture>
      <source media="(prefers-color-scheme: dark)" srcset="https://api.star-history.com/chart?repos=whycorporation/whycodes&type=date&theme=dark&legend=top-left" />
      <source media="(prefers-color-scheme: light)" srcset="https://api.star-history.com/chart?repos=whycorporation/whycodes&type=date&legend=top-left" />
      <img alt="Star History Chart" src="https://api.star-history.com/chart?repos=whycorporation/whycodes&type=date&legend=top-left" />
    </picture>
  </a>
</p>

## 许可证

[MIT](LICENSE) · [why.codes](https://why.codes) · [X @whycodesai](https://x.com/whycodesai) · [赞助](https://github.com/sponsors/whycorporation)
