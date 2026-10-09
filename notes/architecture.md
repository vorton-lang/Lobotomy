# 架构

> Status: 2026-10-10。取代 M1 实现计划（原 `m1-plan.md`）：仍然有效的代码布局与技术选择移到这里，各增量的进度记录留在 git 历史中。
> 依据：[data-model.md](data-model.md)、[harness-adapter.md](harness-adapter.md)、[frontend.md](frontend.md)。

## 1. 进程

```text
Electron 主进程（窗口、托盘）──启动──→ lobotomyd：后端，常驻，每个 host 目录一个
Electron 渲染进程 ──WebSocket /gui──→ lobotomyd
                                       ├─ host：项目登记、额度域、权限模式、CLI 配置（host.db）
                                       └─ 项目实例 × N：SQLite、jj 存储、文件库、槽位、调度器
role 的 CLI（每个 turn 一个进程）──MCP /mcp/{project}/{token}──→ lobotomyd
```

- 这是 M3 的结构：一个后端进程管理所有项目（data-model.md §10，frontend.md §1）。
- M2 时的现状：一个后端进程只开一个项目，Electron 按 host 目录中 `gui.json` 记下的项目启动后端；MCP 地址是 `/mcp/{token}`。
- role 的 CLI 每个 turn 启动一次，结束即退出（harness-adapter.md §1.1）。

## 2. 代码布局

```text
backend/                 Rust workspace
  crates/core            业务对象、SQLite schema 与迁移、命令层（事务、幂等、事件序号）、GUI 的读模型（view）、host.db
  crates/store           jj 存储的封装：接入、物化、采集、pin、rebase、预览。jj 的类型不出这个 crate
  crates/harness         Claude 与 Codex 的 adapter：参数、事件解析；平台层（Windows Job、控制台、句柄）
  crates/lobotomyd       后端进程：host 与项目实例、调度器、turn runner、采集与验证、GUI WebSocket、MCP HTTP
    tests/fixtures       假 Codex、假 Claude（fake-*.mjs）
frontend/                Electron + React + TypeScript
  src/                   渲染进程
  electron/              主进程
  e2e/                   Playwright 端到端测试
  bench/                 性能基线（perf-baseline.md）
spikes/                  实测用的探针脚本
```

## 3. 技术选择

| 用途 | 选择 | 理由 |
|---|---|---|
| SQLite 访问 | rusqlite（bundled） | 命令层持有短事务并向下传递（#6 §1），同步的 `Transaction` 正好合适。单写者、本地进程，不需要 SQLx 的异步连接池和编译期查库 |
| 写入方式 | 每个项目一个写连接，WAL 模式；读用单独的只读连接（data-model.md §1）；命令在 `spawn_blocking` 中执行 | 单写者避免事务交错 |
| jj | jj-lib，版本锁定为 `=0.45.1` | API 不稳定，锁定版本并封装在 `store` 中。只用 jj 的存储（git backend）、树合并和本地工作副本状态，不用操作日志与视图：业务事实在 SQLite 中，成果靠 pin 保留 |
| 异步与 HTTP | tokio、axum | GUI 的 WebSocket 与 MCP 的 HTTP 由同一个服务提供（frontend.md §1） |
| MCP | 官方 Rust SDK rmcp 3.5，封装在 lobotomyd 的 `mcp` 模块中，类型不出模块 | MCP 2026-07-28 版协议改为无状态：去掉会话与 `initialize` 握手，每个请求在 `_meta` 中带协议版本与能力。rmcp 同时支持新旧版本，默认校验 `Host`（只允许 loopback）。工具处理函数可以从请求上下文读到 URL 中的 token（harness-adapter.md §2） |
| ID | 类型前缀加 ULID，例如 `task_01J…`，同一毫秒内也按生成顺序排序 | data-model.md §7.7 |
| Windows 进程 | windows-sys | Job Object、`CREATE_NO_WINDOW`、挂起创建、Ctrl+C 辅助进程（harness-adapter.md §1.8） |
| 调试信息 | 自己的代码只保留行号表，依赖不带调试信息（`backend/Cargo.toml`） | 2026-10-09：`target/` 从约 37 GB 降到约 3 GB，panic 和回溯仍有行号 |
| 前端 | React、Vite、zustand；组件选型见 frontend.md §4.1 | 渲染进程也能在浏览器中开发 |

项目范围内的状态都挂在项目实例上，不使用全局单例；跨项目的状态放在 host（data-model.md §10）。

## 4. 测试

改什么跑什么，见 [AGENTS.md](../AGENTS.md)"测试：改什么跑什么"。

- **CI**（用户确认，2026-10-04）：每次推送与 PR 都在 Windows 和 Ubuntu 上运行格式检查、全部测试与 Clippy；推送到 main 时另外运行性能基线，只记录，不设门槛（`.github/workflows/ci.yml`）。后端按 `backend/rustfmt.toml` 统一格式（行宽 120）。

- 后端测试用假 CLI 驱动真实的后端，覆盖运行时的各条路径。真实 CLI 的测试消耗订阅额度，默认不运行（harness-adapter.md §2）。
- 前端的单元测试与端到端测试见 frontend.md §7"测试"。
- 把后端的几个测试入口合并为一个二进制，试过两次，运行时间没有明显缩短，构建反而更慢，已撤回（[#24](https://github.com/vorton-lang/Lobotomy/pull/24)）。
