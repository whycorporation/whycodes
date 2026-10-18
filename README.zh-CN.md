<p align="center">
  <img src="assets/banner.svg" alt="whycodes — 面向终端的快速、与供应商无关的编程智能体，使用 Rust 编写" width="720">
</p>

<p align="center">
  <a href="https://github.com/whycorporation/whycodes/actions/workflows/ci.yml"><img src="https://github.com/whycorporation/whycodes/actions/workflows/ci.yml/badge.svg" alt="CI"></a>
  <a href="https://github.com/whycorporation/whycodes/releases"><img src="https://img.shields.io/github/v/release/whycorporation/whycodes" alt="Release"></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-MIT-blue.svg" alt="License: MIT"></a>
  <a href="https://discord.gg/H2KsKYEdT8"><img src="https://img.shields.io/badge/Discord-join-5865F2?logo=discord&logoColor=white" alt="加入 WhyCodes Discord"></a>
</p>

<p align="center">
  <a href="README.md">English</a> · 简体中文 · <a href="README.ru.md">Русский</a> · <a href="README.es.md">Español</a>
</p>

**面向终端的快速、与供应商无关的编程智能体，使用 Rust 编写。**

WhyCodes 可读写并编辑文件、运行命令、搜索工作区，并通过多轮工具调用驱动
大语言模型 —— 既有全屏 TUI，也可作为机器可读的一次性 CLI。它强调较小的
原生体积、空闲时高效的 TUI，以及不绑定任何单一模型供应商的工作流。

<p align="center">
  <img src="assets/tui-demo.svg" alt="whycodes TUI：从主屏幕到完成一次包含搜索、编辑和测试的对话" width="820">
</p>

## 为什么选择 WhyCodes

- **即刻启动。** 在 Linux 上 `whycodes --version` 1.4 ms 即返回。
- **空闲时零开销。** TUI 仅在有变化时绘制：空闲会话每秒绘制 0 帧。
- **体积小巧。** 十个空闲会话共占 41.9 MB，每多一个约 3.1 MB；恢复一个
  3000 条消息的会话占用 34.7 MB。
- **使用你选择的模型。** 托管 API、本地模型服务器或任何 OpenAI 兼容端点
  —— 可按每次运行或在会话中途切换。

在同一台机器上测得；方法与注意事项见[性能](#性能)。

## 亮点

- **单一原生二进制。** 无需安装其他任何东西：搜索在进程内完成；在 Windows
  上命令无需 WSL 即可运行（若已安装 Git for Windows 则通过它运行，否则使用
  内嵌的 bash 兼容 shell）。
- **跨平台。** Linux、macOS 与 Windows 使用同一套工作流。
- **任意供应商。** Anthropic、OpenAI、Google、GitHub Copilot、Groq、xAI、
  DeepSeek、Ollama、OpenRouter、Mistral、Together，LM Studio、vLLM、
  llama.cpp 等本地服务器，或任何 OpenAI 兼容端点。
- **贴合你的项目。** 从 `AGENTS.md` 加载项目说明，并通过 LSP 连接已有的
  MCP 工具与语言服务器。
- **记住你的工作。** 可分支与恢复的持久化会话、跨会话记忆（`MEMORY.md`、
  语义召回）以及可选的代码索引。

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

<details>
<summary>Homebrew 提示该 tap 未受信任</summary>

Homebrew 6+ 会拒绝未受信任的第三方 tap。上面使用完全限定名称的安装命令只
信任本 formula。若已经 tap 并看到该错误：

```bash
brew trust --formula whycorporation/whycodes/whycodes
brew install whycodes
```

</details>

### 从源码构建

```bash
# 需要 libsqlite3（pkg-config）。完全静态二进制：
# cargo build --release -p whycodes-cli --features bundled-sqlite
cargo build --release -p whycodes-cli
```

使用 `whycodes upgrade` 更新（脚本 / cargo 安装），或 `brew upgrade
whycodes`（Homebrew）。交互式 TUI 会话会检查 GitHub 上的新版本，并在主
屏幕安装前提示；传入 `--no-auto-update` 可跳过检查。安装脚本会对照已发布
的 `SHA256SUMS` 校验发行产物。可下载二进制与卸载说明见
[docs/packaging.md](docs/packaging.md)。Windows 发行二进制的签名方式见
[Code signing policy](docs/code-signing-policy.md)。

配置和会话位于 `~/.whycodes`（Windows 上为 `%USERPROFILE%\.whycodes`），
若设置了 `$WHYCODES_HOME` 则位于该目录。

<details>
<summary>从 0.6.4 或更早版本升级</summary>

0.6.5 将配置和会话从此前使用的平台目录移出（Linux 为 `~/.config/whycodes`
或 `~/.config/com.whycorporation.whycodes`，macOS 为
`~/Library/Application Support/com.whycorporation.whycodes`，Windows 为
`%APPDATA%\whycorporation\whycodes`）。新目录为空时，首次启动会复制
`config.toml`、`auth.json` 和 `whycodes.db`，并保留旧文件。

</details>

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
whycodes -P openai -m gpt-5 generate "Refactor this module"
```

没有 API 密钥？在本地运行模型：

```bash
whycodes -P ollama -m qwen2.5-coder               # Ollama
whycodes provider discover --save                 # 查找 LM Studio、vLLM、llama.cpp
```

订阅 OAuth 登录（`whycodes auth login <provider>`）仅在安装本地
`kind: "auth"` 插件后可用 —— WhyCodes 不附带第三方 OAuth 客户端。见
[docs/auth.md](docs/auth.md)。

完整指南 —— CLI 参考、TUI 快捷键、斜杠命令、智能体、工具与配置 —— 见
**[docs/guide.md](docs/guide.md)**。

## 功能

| | |
|---|---|
| **智能体** | `build`（完整权限）、`plan` 与 `ask`（只读），以及能解释并修复自身配置的 `whycodes`；通过 `task` 工具使用 `general`、`explore`、`scout` 子智能体，可选返回类型化结果；通过 `swarm` 在 git worktree 中并行工作，可在智能体中心（`/agents`）查看与引导 |
| **模型** | Prewalk 在强模型上规划一轮，再把编辑交给更便宜的模型；可按每次运行或在会话中途切换供应商或模型（`/models`）；可选启用 Anthropic 的供应商原生压缩 |
| **工具** | 文件编辑/补丁、以单元格方式处理 Jupyter 笔记本、进程内搜索、shell、git 与 GitHub、网页抓取/搜索、CDP 驱动的浏览器、带 `wait` 的后台任务、调度与待办跟踪；启用 `ast-tools` 特性构建时提供 `ast_grep` / `ast_edit` |
| **会话** | 按项目持久化；用 `--continue` / `--resume` 恢复，用 `/branch` 与 `/tree` 分支并回到之前的轮次，从其他智能体 CLI 导入记录，通过本地服务器分享 |
| **引导** | 将消息排队到本轮结束后（`/queue`），或在工具调用之间打断（`/now`）；输出匹配时打断模型的流规则（`whycodes rules`） |
| **记忆** | 可人工编辑的 `MEMORY.md`、带嵌入的语义事实、可选代码 RAG 索引 —— 均按项目、均可选 |
| **无界面 / CI** | `generate` 支持 `--format json` 或 `stream-json`（NDJSON），多个提示可并发运行，失败时非零退出；`whycodes slop` 报告 diff 的 ΔLOC / 冗长度 / 侵蚀度 |
| **安全** | 审批预设（`--approval-mode auto\|important\|manual`）、按工具的权限门、shell 命令风险分析、可选操作系统沙箱（bubblewrap）、HTTP 域名白名单与工具钩子 |
| **扩展** | MCP 服务器（stdio 与 HTTP）、LSP 语言服务器、技能、shell 插件、自定义斜杠命令、主题 |
| **嵌入** | 基于守护进程协议 v1（`whycodes serve`）的 Rust 与 TypeScript SDK |

## 性能

2026-10-16 在同一台机器（Ryzen 7 3800X）上测量：Windows 11 原生运行，
Linux 运行于 WSL2。方法、历史数据与注意事项见
[docs/benchmarks.md](docs/benchmarks.md)：

| 指标 | Windows | Linux（WSL2） |
|---|---|---|
| `--version` | **12.9 ms** | **1.4 ms** |
| 首帧（harness，进程内） | **0.25 ms** | **0.08 ms** |
| 空闲重绘（3 s） | **0.0 /s** | **0.0 /s** |
| 1 个空闲会话 PSS | —（仅 `/proc`） | **13.6 MB** |
| 10 个空闲会话 PSS | — | **41.9 MB**（每多一个约 3.1 MB） |
| 恢复 3000 条消息的会话 PSS | — | **34.7 MB** |

TUI 仅在有变化时绘制，空闲会话不会重绘。恢复一个 3000 条消息的会话占用
34.7 MB，十月的内存优化之前为 216 MB。首帧 harness 只计时一次启动画面绘制，
而非完整主屏幕。

工作区行覆盖率低于 98.5% 时 CI 失败，二十四个 crate 的生产代码行覆盖率保持
100% —— 见 [docs/coverage.md](docs/coverage.md)。

## 文档

| 文档 | 内容 |
|---|---|
| [docs/guide.md](docs/guide.md) | 用法：CLI、TUI、智能体、工具、配置、SDK |
| [docs/tools/](docs/tools/README.md) | 每个内置工具一页 |
| [docs/config.md](docs/config.md) | 每个配置项及其默认值与环境变量覆盖 |
| [docs/providers.md](docs/providers.md) | 供应商目录：特殊行为、认证、模型处理 |
| [docs/auth.md](docs/auth.md) | API 密钥、OAuth、凭据导入 |
| [docs/architecture.md](docs/architecture.md) | Crate 地图与分层 |
| [docs/internals/](docs/internals/) | 轮次循环、权限门、压缩、提示缓存、重试、MCP |
| [docs/roadmap.md](docs/roadmap.md) | 当前重点与延后工作 |
| [docs/knowhow.md](docs/knowhow.md) | 来之不易的缺陷（TUI、tty、静默退出） |
| [docs/tui-term-matrix.md](docs/tui-term-matrix.md) | 在 Alacritty / Kitty / VTE 上的手动 TUI 检查 |
| [docs/benchmarks.md](docs/benchmarks.md) | 测量启动、RSS、空闲绘制 |
| [docs/coverage.md](docs/coverage.md) | 测量行覆盖率 |
| [docs/budgets.md](docs/budgets.md) | CI 质量预算 |
| [docs/packaging.md](docs/packaging.md) | Homebrew、安装器、发行下载次数 |
| [docs/code-signing-policy.md](docs/code-signing-policy.md) | Code signing policy（代码签名政策） |

## 贡献

欢迎贡献。[CONTRIBUTING.md](CONTRIBUTING.md) 是从克隆到合并变更的短路径；
[AGENTS.md](AGENTS.md) 约束在本仓库中工作的编程智能体。参与即表示同意
[行为准则](CODE_OF_CONDUCT.md)。帮助与缺陷报告：[SUPPORT.md](SUPPORT.md)。
请通过 [SECURITY.md](SECURITY.md) 报告漏洞，不要开公开 issue。

## 社区

<p>
  <a href="https://discord.gg/H2KsKYEdT8"><img src="https://img.shields.io/badge/Discord-join-5865F2?logo=discord&logoColor=white" alt="加入 WhyCodes Discord"></a>
  <a href="https://t.me/whycodesai"><img src="https://img.shields.io/badge/Telegram-whycodesai-26A5E4?logo=telegram&logoColor=white" alt="WhyCodes Telegram"></a>
  <a href="https://x.com/whycodesai"><img src="https://img.shields.io/badge/X-@whycodesai-black" alt="在 X 上关注 @whycodesai"></a>
  <a href="https://www.youtube.com/@whycodesai"><img src="https://img.shields.io/badge/YouTube-@whycodesai-FF0000?logo=youtube&logoColor=white" alt="WhyCodes YouTube"></a>
  <a href="https://olud.ai/tool/whycodes.html"><img src="https://olud.ai/badge.php?tool=whycodes" alt="olud.ai"></a>
</p>

网站：[why.codes](https://why.codes)。

## 赞助

WhyCodes 独立开发，需要资金才能持续发布。请通过
[GitHub Sponsors](https://github.com/sponsors/whycorporation) 赞助本项目。

<a href="https://github.com/sponsors/whycorporation"><img src="https://img.shields.io/badge/sponsor-whycorporation-ea4aaa" alt="赞助 WhyCodes"></a>

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

[MIT](LICENSE)
