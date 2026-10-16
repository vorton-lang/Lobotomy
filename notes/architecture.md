# 架构

> Status: 现行的进程结构、代码布局与技术选择。
> 依据：[data-model.md](data-model.md)、[harness-adapter.md](harness-adapter.md)、[frontend.md](frontend.md)。

## 1. 进程

```text
Electron 主进程（窗口、托盘）──启动──→ lobotomyd：后端，常驻，每个 host 目录一个
Electron 渲染进程 ──WebSocket /gui──→ lobotomyd
                                       ├─ host：项目登记、额度域、权限模式、CLI 配置（host.db）
                                       ├─ 项目实例 × N：SQLite、文件库、role 的工作目录、调度器
                                       └─ Angela 实例 × 1：她自己的 SQLite、文件库、调度器（host 目录下的 angela/）
项目中 role 的 CLI（每个 turn 一个进程）──MCP /mcp/{project}/{token}──→ lobotomyd
Angela 的 CLI（每个 turn 一个进程）──MCP /mcp/angela/{token}──→ lobotomyd
```

**语义更新**（[#86](https://github.com/vorton-lang/Lobotomy/issues/86)，用户确认，2026-10-17）：`store` crate（jj 私有存储）、采集、验证、预览、展示分支采用与试用终端随 worker 层的替换退出。

- 一个后端进程管理所有项目和 Angela（data-model.md §10，frontend.md §1）。Angela 打不开时，项目照常运行。
- lobotomyd 的模块：
  - `host.rs`：跨项目的状态。
  - `instance.rs`：运行 turn、保存对话记录的实例，是项目实例与 Angela 共有的部分：数据库、文件库、运行中的 turn 与它们的实时条目。runner、GUI 的推送和对话记录的读取对两者相同。
  - `project.rs`：一个项目实例，现在只包着它的 `Instance`。
  - `projects.rs`：从登记中打开项目、新建并接入项目、记下打不开的项目，以及停止时逐个停下项目。
  - `angela.rs`：Angela 的实例与调度器。调度器观察各项目，按规则为她登记 turn，规则见 manager-actions.md §6"Manager 什么时候运行"；她的数据位置见 data-model.md §10.6。
  - `mcp`：MCP 服务。`mod.rs` 是项目中 role 的工具，`angela.rs` 是 Angela 的工具。
  - `launch.rs`：后端启动的所有进程都经过它（harness-adapter.md §1.8）。
- role 与 Angela 的 CLI 每个 turn 启动一次，结束即退出（harness-adapter.md §1.1）。

## 2. 代码布局

```text
backend/                 Rust workspace
  crates/core            业务对象、SQLite schema 与迁移、命令层（事务、幂等、事件序号）、GUI 的读模型（view）、host.db
  crates/harness         Claude 与 Codex 的 adapter：参数、事件解析；平台层（Windows 的 Job、控制台、句柄，Linux 的进程组）
  crates/lobotomyd       后端进程：host、项目实例与 Angela、调度器、turn runner、GUI WebSocket、MCP HTTP
    examples/            export-gui-types.rs：从 Rust 类型导出 GUI 的 TypeScript 类型（需 ts-rs feature）
    tests/fixtures       假 Codex、假 Claude（fake-*.mjs）
frontend/                Electron + React + TypeScript
  src/                   渲染进程；src/api/generated.ts 是生成的 GUI 类型（frontend.md §7.1）
  electron/              主进程
  scripts/               api-types.mjs：`npm run api:generate` 生成类型，`npm run api:check` 检查漂移
  e2e/                   Playwright 端到端测试
  bench/                 性能基线（perf-baseline.md）
spikes/                  实测用的探针脚本
```

## 3. 技术选择

| 用途 | 选择 | 理由 |
|---|---|---|
| SQLite 访问 | rusqlite（bundled） | 命令层持有短事务并向下传递（#6 §1），同步的 `Transaction` 正好合适。单写者、本地进程，不需要 SQLx 的异步连接池和编译期查库 |
| 写入方式 | 每个项目一个写连接，WAL 模式；读用单独的只读连接（data-model.md §1）；命令在 `spawn_blocking` 中执行 | 单写者避免事务交错 |
| 异步与 HTTP | tokio、axum | GUI 的 WebSocket 与 MCP 的 HTTP 由同一个服务提供（frontend.md §1） |
| MCP | 官方 Rust SDK rmcp 3.5，封装在 lobotomyd 的 `mcp` 模块中，类型不出模块 | MCP 2026-07-28 版协议改为无状态：去掉会话与 `initialize` 握手，每个请求在 `_meta` 中带协议版本与能力。rmcp 同时支持新旧版本，默认校验 `Host`（只允许 loopback）。工具处理函数可以从请求上下文读到 URL 中的 token（harness-adapter.md §2） |
| ID | 类型前缀加 ULID，例如 `task_01J…`，同一毫秒内也按生成顺序排序 | data-model.md §7.7 |
| Windows 进程 | windows-sys | Job Object、`CREATE_NO_WINDOW`、挂起创建、Ctrl+C 辅助进程（harness-adapter.md §1.8） |
| 调试信息 | 自己的代码只保留行号表，依赖不带调试信息（`backend/Cargo.toml`） | 2026-10-09：`target/` 从约 37 GB 降到约 3 GB，panic 和回溯仍有行号 |
| GUI 类型 | ts-rs，版本锁定为 `=12.0.1`；只在可选的 `ts-rs` feature 中编译，默认构建不含 | GUI 协议的 TypeScript 类型由 Rust 类型生成，CI 检查生成结果没有漂移（frontend.md §7.1） |
| 前端 | React、Vite、zustand；组件选型见 frontend.md §4.1 | 渲染进程也能在浏览器中开发 |

项目范围内的状态都挂在项目实例上，不使用全局单例；跨项目的状态放在 host（data-model.md §10）。

## 4. 测试

改什么跑什么，见 [AGENTS.md](../AGENTS.md)"测试：改什么跑什么"。

- **CI**（用户确认，2026-10-04；`.github/workflows/ci.yml`）：每次推送与 PR 都在 Windows 和 Ubuntu 上运行：
  - 后端：格式检查、全部测试、Clippy。Clippy 跑两次：默认构建一次，加 `--features lobotomyd/ts-rs` 一次。
  - GUI 类型：`npm run api:check`，检查生成的类型没有漂移，并用真实的 serde 输出检查类型（frontend.md §7.1）。
  - 前端：类型检查、单元测试、构建。
  - 端到端测试：Windows 上用本机 Edge，Ubuntu 上用 Playwright 自带的 Chromium，Electron 在 Xvfb 下运行。
  - 推送到 main 时另外运行性能基线，只记录，不设门槛（perf-baseline.md）。
- 后端按 `backend/rustfmt.toml` 统一格式（行宽 120）。
- **语义更新**（[#86](https://github.com/vorton-lang/Lobotomy/issues/86)，用户确认，2026-10-17）：只保留回归测试，没有拦住过问题的测试已经退役（AGENTS.md"测试的目的"）。
- 后端测试用假 CLI 驱动真实的后端。真实 CLI 的测试已随测试退役删除，留下的实测结果见 harness-adapter.md §2。
- 前端的单元测试与端到端测试见 frontend.md §7.13。
- 把后端的几个测试入口合并为一个二进制，试过两次，运行时间没有明显缩短，构建反而更慢，已撤回（[#24](https://github.com/vorton-lang/Lobotomy/pull/24)）。
