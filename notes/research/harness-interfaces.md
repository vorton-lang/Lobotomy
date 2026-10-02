# 调研：harness 接入方式与条款

> 调研快照（2026-10-01），事实可能已经变化；采用前请重新核实。
> 设计决定见 [harness-adapter.md](../harness-adapter.md)。本文记录决定所依据的调研结果。

## 1. 决定与理由

- **Adapter 只调用官方 CLI**，不使用 Claude Agent SDK、Codex SDK / app-server 或 ACP adapter。
  - 用户的判断：SDK 容易出各种问题，CLI 界面相对稳定，而且按正常方式调用 CLI 与用户手动使用一致。
  - 两个 ACP adapter 本身就封装在原生接口之上（Claude 的基于 Agent SDK，Codex 的基于 app-server）。走 ACP 并不减少对原生接口的依赖，还要多承担一层第三方代码。
  - ACP 缺少对 Lobotomy 必需的能力：没有注入角色指令的标准方式，fork 和压缩事件也不稳定（见 §2）。
- **多 harness 仍需要内部抽象。** 内部接口的会话语义可以向 ACP v1 看齐，以后给长尾 harness 加一个通用的 ACP adapter。

## 2. ACP 现状

- 线协议 v1；schema / SDK 包 1.24.1（2026-09-30）；v2 自 2026-07-20 起为草案。[changelog](https://github.com/agentclientprotocol/agent-client-protocol/blob/main/CHANGELOG.md)、[v2 draft](https://agentclientprotocol.com/announcements/acp-v2-draft)
- **稳定**：
  - `initialize`、`authenticate`；
  - `session/new | load | list | resume | close | delete | prompt | cancel | set_mode | set_config_option`；
  - `request_permission`；`fs/*`、`terminal/*`；
  - `usage_update`（只有会话级用量）。
- **不稳定**：`session/fork`、按轮用量、`mcp/message`；压缩通知（`compaction_update`）2026-09-23 起处于预览。[schema](https://agentclientprotocol.com/protocol/schema)、[compaction RFD](https://agentclientprotocol.com/rfds/session-compaction)
- **没有** system prompt / 角色指令的标准字段。模型切换走 config option。
- **Claude adapter**：`@agentclientprotocol/claude-agent-acp`，基于 Agent SDK，版本 v0.84.0。角色指令走私有字段 `_meta.systemPrompt`。fork 有 bug：fork 出的会话无法发消息（#1110）。Claude Code 不原生支持 ACP（[#6686](https://github.com/anthropics/claude-code/issues/6686) 已以 not planned 关闭）。[repo](https://github.com/agentclientprotocol/claude-agent-acp)
- **Codex adapter**：`@agentclientprotocol/codex-acp`（Zed 的原版已于 2026-07-22 归档），TypeScript 实现，基于 app-server，版本 v2.1.0。不支持传入角色指令（#215 未关）。[repo](https://github.com/agentclientprotocol/codex-acp)
- **使用 ACP 的多 harness 客户端**：Zed、JetBrains（Air 并行运行 Claude、Codex、Gemini CLI 和 Junie）、OpenHands Agent Canvas、`openclaw/acpx`。

## 3. 原生接口（未采用，留作参考）

- **Claude Agent SDK**：流式输入、resume / fork / 自定 sessionId、`interrupt()`、`setModel()`、`canUseTool`、hooks（含 PreCompact / PostCompact）、进程内 MCP、`getContextUsage()`、`compact_boundary`。默认使用自带的二进制，可用 `pathToClaudeCodeExecutable` 指向本机安装的 `claude`。[docs](https://code.claude.com/docs/en/agent-sdk/typescript)
- **Codex app-server**：
  - stdio 上的 JSON-RPC；
  - `thread/start | resume | fork | compact`、`turn/start | steer | interrupt`；
  - 审批请求；`tokenUsage/updated`（含上下文窗口）；
  - 支持 `baseInstructions` / `developerInstructions`；
  - 官方标注"实验性，不支持生产负载"。[docs](https://learn.chatgpt.com/docs/app-server)
- **Codex 其他接口**：
  - `codex exec --json` 与 `exec resume` 中途无法应答审批；
  - TypeScript SDK 封装的是 `exec`；Python SDK 封装的是 app-server；
  - `codex mcp-server` 已在 v0.154.0 移除。

## 4. 条款

- **Anthropic**（[legal-and-compliance](https://code.claude.com/docs/en/legal-and-compliance)）：
  - 订阅 OAuth 面向"普通使用 Claude Code 及原生应用"；
  - 构建产品的开发者（包括使用 Agent SDK 的）应使用 API key；
  - 不允许第三方产品提供 claude.ai 登录，或代用户使用订阅额度；
  - 同时**允许最终用户用自己的订阅登录未修改的 Claude Code 二进制**；
  - Pro / Max 的额度按"普通的个人使用"设计。
- **OpenAI**（[app-server](https://learn.chatgpt.com/docs/app-server)、[Sign in with ChatGPT](https://developers.openai.com/siwc/token-sharing-open-source)）：
  - 本地 / 开源应用可以继续使用 app-server 认证，推荐迁移到 Sign in with ChatGPT；
  - 商业或托管服务不允许。
- **Lobotomy 的立场**：按正常方式调用用户自己安装的未修改 CLI，由用户自行登录，Lobotomy 不接触凭据。
- **剩余风险**：多个长期会话并发可能超出"普通个人使用"的额度预期。同时运行的 role 数量需显式可控。

## 5. 本机观察（2026-10-01 / 02）

- **Codex 桌面应用**：
  - Electron（Forge + Vite），打包目标包括 deb、rpm、msix；
  - 依赖 `app-server-manager`、`vscode-jsonrpc`、`capnweb`，即 Electron 前端加独立的 Rust app-server；
  - 从渲染层打包产物的字符串推断：TanStack Virtual、micromark / mdast、Shiki、CodeMirror 6、xterm.js、yjs，以及大量 Web Worker；
  - CLI 位于 `%LOCALAPPDATA%\OpenAI\Codex\bin\<hash>\`，应用更新后路径会变（一天内从 26.928.2636 升到 26.928.3736）。
- **Claude 桌面应用**：Electron + Vite + React + Tailwind，带有自建的 `findInPage` 窗口和 `transcript-search-worker`。
- **role 会话默认继承的对外通道**：
  - Claude：claude.ai 连接器（Docs 已连接，Slack、Drive 待授权）；
  - Codex：ChatGPT 账号的 apps 连接器，包括 GitHub 写操作、Outlook 发信、Drive 分享、Sites 部署、computer-use、浏览器控制。
  - 裁剪方法与实测结果见 [harness-adapter.md](../harness-adapter.md) §1.6。
