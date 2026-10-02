# 调研：agent GUI 产品的前端做法与踩坑

> 调研快照（2026-10-02），事实可能已经变化。设计决定见 [frontend.md](../frontend.md)，本文记录依据。

## 1. 产品概览

| 产品 | 技术栈 | 列表渲染 | 大输出处理 | 与后端的连接 |
|---|---|---|---|---|
| Goose desktop | Electron、React 19、react-markdown + Prism | 未虚拟化，分批挂载；首 20 / 尾 200 窗口在 PR [#11994](https://github.com/aaif-goose/goose/pull/11994) | Rust 侧截断并把大输出写到文件（[#12110](https://github.com/aaif-goose/goose/issues/12110)） | 原为 REST + SSE（`Last-Event-ID`），现为 WebSocket 上的 ACP |
| OpenHands Agent Canvas | Electron 43、React 19、zustand、xterm 6、Monaco | 冷启动加载最近 50 条；虚拟化在 PR [#17820](https://github.com/OpenHands/OpenHands/pull/17820) | xterm 面板；后端卸载大输出 | REST 历史 + WebSocket 按时间戳重发 |
| opencode desktop | Electron 42 + SolidJS（原 Tauri） | `@tanstack/solid-virtual`，从最新开始分页 | shell 输出保留尾部 64 KiB；`@pierre/diffs` | HTTP + SSE，10 秒心跳 |
| Kilo Code | Solid、virtua、marked 在 Web Worker 中运行 | virtua，游标分页，每页 80 条 | 500 行 / 5 万字符（头 20% 尾 80%），全文写到文件 | HTTP + SSE |
| Cline | React 18、react-virtuoso | Virtuoso | 行折叠；后端在超过 4.8 万字符时省略中段 | postMessage 上的 protobuf，消息带 ts / seq / epoch |
| t3code | Electron、React 19、`@legendapp/list` | 虚拟化，进行中的 turn 不参与（[#118](https://github.com/pingdotgg/t3code/pull/118)） | `@pierre/diffs` | Node WebSocket 服务，事件溯源 SQLite |
| LobeHub | React 19、Electron | 从 Virtuoso 迁到 virtua（[#10151](https://github.com/lobehub/lobehub/pull/10151)） | 工具结果展开时才加载 | SSE + WebSocket 从 `lastEventId` 重放 |
| Cherry Studio v2 | Electron，SQLite 在主进程 | virtua：封存历史 + 实时尾部 | 代码按行虚拟化，Shiki 在 Worker 中，超过 100 KB 显示为纯文本 | IPC；每次运行 1 万块缓冲，重新挂接时重放 |
| LibreChat | React 18、react-markdown | 未虚拟化，分帧挂载 | 超过 20 行的工具输出截到 15 行 | 可恢复 SSE：带序号的增量，快照 + 尾部 |
| Zed | GPUI（Rust 原生） | GPUI `ListState` | Markdown 在 UI 线程外解析，高亮结果缓存（[#63138](https://github.com/zed-industries/zed/pull/63138)） | 进程内 |
| Warp | Rust、GPU 渲染 | 块高度用 SumTree 索引，逐块、逐行虚拟化（[blog](https://www.warp.dev/blog/block-model-behind-warps-agentic-development-environment)） | 紧凑字节缓冲 + 行偏移索引 | 进程内 |

另有 Roo Code（已归档）、Continue、Open WebUI、Jan、Cursor、Conductor、Claude desktop、Codex app，见各自仓库与 changelog。

## 2. 踩坑与修复

- **每个 token 都对整条消息重做工作，开销随长度平方增长**：Goose [#10075](https://github.com/aaif-goose/goose/issues/10075) → [#11583](https://github.com/aaif-goose/goose/pull/11583)；LibreChat [#14332](https://github.com/LibreChat-AI/LibreChat/pull/14332)：每帧只写一次缓存，单条回复的渲染次数从 5.7 万降到 4200。
- **打开长会话时整段重放**：Goose [#10665](https://github.com/aaif-goose/goose/pull/10665)：188.5 秒 → 4.3 秒；[#11559](https://github.com/aaif-goose/goose/issues/11559)：有界队列溢出后悄悄断开订阅，会话从此打不开；[#12202](https://github.com/aaif-goose/goose/pull/12202)：只重放最近 200 条。
- **DOM 无限增长**：OpenHands [#17820](https://github.com/OpenHands/OpenHands/pull/17820)：800 个事件时，节点数 10,368 → 822。
- **虚拟化库自身引入的问题**：opencode [#42825](https://github.com/anomalyco/opencode/pull/42825)：积累 37,500 个已脱离页面的节点；Roo [#5601](https://github.com/RooCodeInc/Roo-Code/issues/5601)：overscan 设为无穷；Goose [#11348](https://github.com/aaif-goose/goose/issues/11348)：把消息变高误判为用户上滚。修复做法见 Cherry [#16861](https://github.com/CherryHQ/cherry-studio/pull/16861)、LobeHub [#14086](https://github.com/lobehub/lobehub/pull/14086)。
- **推送整个状态或每片都落库**：Cline [#12419](https://github.com/cline/cline/issues/12419)、[#14729](https://github.com/cline/cline/pull/14729)（数据库涨到 1.66 GB）；t3code [#5110](https://github.com/pingdotgg/t3code/issues/5110)：一条回复写出 3024 条 SQLite 事件；Open WebUI [#23733](https://github.com/open-webui/open-webui/issues/23733)。
- **多 agent 并行流式输出**：Kilo [#9057](https://github.com/Kilo-Org/kilocode/pull/9057)：当前会话 16ms 刷新一次，后台会话 150–400ms；Claude Code 扩展 [#68508](https://github.com/anthropics/claude-code/issues/68508)：未节流时界面落后约 60 秒。
- **超大输出**：Goose [#12464](https://github.com/aaif-goose/goose/issues/12464)；opencode [#47926](https://github.com/anomalyco/opencode/pull/47926)；Kilo [#7102](https://github.com/Kilo-Org/kilocode/pull/7102)：同步 Shiki 阻塞主线程 2.3 秒；Cherry [#20199](https://github.com/CherryHQ/cherry-studio/pull/20199)。
- **重连**：Goose [#7834](https://github.com/aaif-goose/goose/pull/7834)、[#8846](https://github.com/aaif-goose/goose/pull/8846)；OpenHands [#17619](https://github.com/OpenHands/OpenHands/issues/17619)；opencode [#51871](https://github.com/anomalyco/opencode/pull/51871)：心跳与卡死检测；LibreChat [#14612](https://github.com/LibreChat-AI/LibreChat/pull/14612)：缓冲无上限，涨到 2.1 GiB。
- **状态订阅导致的重渲染**：OpenHands [#13572](https://github.com/OpenHands/OpenHands/pull/13572)：输入延迟 171ms → 22ms。

## 3. 技术要点

- **流式 Markdown**：
  - [AI SDK 缓存块渲染方案](https://ai-sdk.dev/cookbook/next/markdown-chatbot-with-memoization)；
  - [Streamdown](https://github.com/vercel/streamdown)：每次更新都重新切分整条消息，尾部缓存是全局单例；
  - [remend](https://github.com/vercel/streamdown/tree/main/packages/remend)：补全未闭合的语法。
- **虚拟化**：
  - [virtua](https://github.com/inokawa/virtua)；
  - [TanStack Virtual chat 模式](https://tanstack.com/virtual/latest/docs/chat)：2026-05 加入，需 ≥3.17.6；
  - [VirtuosoMessageList](https://virtuoso.dev/message-list/)：需要商业授权。
- **大输出查看**：
  - [CodeMirror 百万行演示](https://codemirror.net/examples/million/)；
  - Monaco 在 300k 行 / 20 MB 以上不做高亮；
  - xterm.js 每个单元格占 12 字节。
- **高亮**：[Shiki 性能指南](https://shiki.style/guide/best-performance)（Worker、JS 引擎、`dispose()`）；[shiki-stream](https://github.com/antfu/shiki-stream)。
- **diff**：[@pierre/diffs](https://pierre.computer/writing/on-rendering-diffs)；`@codemirror/merge` 遇到大文件时会退化为不精确的 diff。
- **搜索**：
  - [content-visibility](https://web.dev/articles/content-visibility)；
  - [hidden=until-found](https://developer.chrome.com/docs/css-ui/hidden-until-found)；
  - [CSS Custom Highlight API](https://developer.mozilla.org/en-US/docs/Web/API/CSS_Custom_Highlight_API)。
- **Electron**：
  - [性能指南](https://www.electronjs.org/docs/latest/tutorial/performance)；
  - [V8 memory cage](https://electronjs.org/blog/v8-memory-cage)：堆上限 4 GB；
  - `backgroundThrottling` 会让后台窗口的动画帧停住。
