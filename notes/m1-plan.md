# M1 实现计划

> Status: 计划 v0.1（2026-10-04）
> 依据：[roadmap.md](roadmap.md) M1、[data-model.md](data-model.md)、[harness-adapter.md](harness-adapter.md)、[frontend.md](frontend.md)。

M1 的链路：用户建任务 → Malkuth 执行 → 采集并固定成果 → 验证 → 用户验收 → 发布 → 物化预览 → GUI 显示。

## 1. 代码布局

```text
backend/                 Rust workspace（独立于 spikes/ 下的探针）
  crates/core            业务对象、SQLite schema 与迁移、命令层（事务、幂等、事件序号）
  crates/store           jj 存储的封装：接入、物化、采集、pin、rebase、预览（jj 类型不出这个 crate）
  crates/harness         Codex adapter：启动、事件解析、中断；平台层（Windows Job、控制台、句柄白名单）
  crates/lobotomyd       后端进程：项目实例、调度器、outbox 执行器、GUI WebSocket、MCP HTTP
frontend/                Electron + React + TypeScript
```

## 2. 技术选择

| 用途 | 选择 | 理由 |
|---|---|---|
| SQLite 访问 | rusqlite（bundled） | 命令层持有短事务并向下传递（#6 §1），同步的 `Transaction` 正好合适。单写者、本地进程，不需要 SQLx 的异步连接池和编译期查库 |
| 写入方式 | 每个项目一个写连接，WAL 模式；命令在 `spawn_blocking` 中执行 | 单写者避免事务交错；读可以另开连接 |
| jj | jj-lib，版本锁定为 `=0.45.1` | API 不稳定，锁定版本并封装在 `store` 中（harness-adapter.md §6） |
| 异步与 HTTP | tokio、axum | GUI 的 WebSocket 与 MCP 的 HTTP 由同一个服务提供（frontend.md §1） |
| MCP | 官方 Rust SDK rmcp（3.5），封装在 lobotomyd 的 MCP 模块中，类型不出模块 | MCP 在 2026-07-28 版协议中改为无状态：去掉会话与 `initialize` 握手，每个请求在 `_meta` 中携带协议版本与能力，新增必需的 `server/discover`。rmcp 同时支持新旧版本，默认校验 `Host`（只允许 loopback）。工具处理函数可以从请求上下文读到 HTTP 请求信息，按 turn 的 token 从 URL 取得。spike 中自写的最简实现只覆盖旧协议，CLI 升级到新协议后会失效（2026-10-04 改选） |
| ID | 类型前缀加 ULID，例如 `task_01J…` | data-model.md §7.7 |
| Windows 进程 | windows-sys | Job Object、`CREATE_NO_WINDOW`、`PROC_THREAD_ATTRIBUTE_HANDLE_LIST`、Ctrl+C 辅助进程（harness-adapter.md §1.8） |
| 前端构建 | Vite、npm | 本机没有 pnpm；渲染进程先能在浏览器中开发 |

项目范围内的状态都挂在项目实例上，不使用全局单例；额度域等跨项目状态放在外层（data-model.md §10）。

## 3. 增量

每个增量结束时都能单独验证，并提交推送。

1. **数据层与命令框架**：workspace、后端进程骨架、项目实例；M1 用到的表与迁移；命令框架（command_record、幂等键、事务、全局事件序号）；任务相关命令（建任务、修改完成条件、发消息、暂停与恢复、调整队列、放弃、重开、开始 attempt）。用单元测试覆盖唯一约束、幂等与前置条件。
2. **Codex adapter 与调度**：先用两家 CLI 对 rmcp 做契约测试，确认各自协商的协议版本、按 turn 的 token 能否取到、JSON 响应能否正常工作。然后实现登记 turn、启动 CLI（Windows 平台层）、事件解析为 item、turn 状态与对账；MCP 服务与 `org_report`；调度器（开始 attempt、绑定消息、登记 turn）；额度域。契约测试保留下来，每次 CLI 或 rmcp 升级后重跑。契约测试已完成，两家都通过（2026-10-04，结果见 harness-adapter.md §2）。

   进度（2026-10-04）：已完成契约测试、native session 与 turn 的命令、`org_report`、item 与 blob 存储、Codex 参数与事件解析、Windows 平台层、turn runner、调度器与启动对账。测试用假 CLI 覆盖完成、失败后继续、中断、重启对账，另有一个真实 Codex turn 的测试（默认不运行）。额度域也已完成：受阻时不开始 attempt、不登记 turn；恢复只由用户手动重试触发，因额度失败的 role 与其他失败一样等用户"继续"（#11 讨论后删除了自动检查与自动继续）。native session 改为按任务划分（#11）。检查调用用 `codex exec --ephemeral`，已对真实 Codex 验证。Codex 被拒的形式仍未知，M1 中不会自动识别额度失败（data-model.md §8.3）。第 2 个增量完成。
3. **成果与发布**：项目接入、槽位物化、每个 turn 的采集（范围规则、体积护栏）、pin；验证（rebase、验证现场、检查命令）；验收事务；预览物化。

   进度（2026-10-04）：第 3 个增量完成。
   - 私有存储在 `backend/crates/store`：jj-lib 0.45.1 的 git backend，直接写提交，pin 是以记录 ID 命名的 ref。
   - 槽位与验证现场都是借用私有存储对象的 git clone。物化前先快照，所以中断的检出原地重做即可补完（harness-adapter.md §3 语义更新）。
   - 采集前的遍历负责体积护栏和 fail closed。
   - 验证用 jj 的树合并做 rebase，检查命令经平台 shell 在 Job Object 中运行。
   - 验收是一个 SQLite 事务；预览快进用户分支。
   - 测试覆盖：
     - store 的集成测试，使用真实 git 仓库；
     - 命令层测试；
     - 用假 CLI 的端到端测试：从建任务、`done`、验证、验收到用户仓库出现提交；检查失败与超时；超过体积护栏；预览遇到本地改动后停止，用户重试；
     - 真实 Codex 的 turn 走到验收（默认不运行）。
   - 待真实项目观察：体积护栏的阈值、换行符（harness-adapter.md §6）。
4. **GUI**：WebSocket 协议（快照加序号、增量、命令）；React 界面（建任务、Malkuth 的 Thread、候选成果的 diff、验收与退回、turn 与额度状态）；Electron 外壳。
5. **端到端与性能基线**：用一个真实的小仓库走通 M1 链路；性能基线框架（frontend.md §5）。
