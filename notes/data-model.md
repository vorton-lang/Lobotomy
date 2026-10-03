# 数据模型：业务对象、命令与对话记录

> Status: 设计中（2026-10-03）
> 按 [#7](https://github.com/vorton-lang/Lobotomy/issues/7) 逐项讨论；已确认的内容写入本文，未讨论的列在 §5。
> 依据：[#6](https://github.com/vorton-lang/Lobotomy/issues/6) §1（SQLite 是业务事实的唯一权威）、#7 及其[讨论补充](https://github.com/vorton-lang/Lobotomy/issues/7#issuecomment-5953401457)、[harness-adapter.md](harness-adapter.md)、[roles-and-tasks.md](roles-and-tasks.md)、[frontend.md](frontend.md) §3。

## 1. 命令

所有状态变化都经后端的命令完成。GUI、调度器和 role（经 MCP）调用同一组命令（#6 §1）。

- 命令按领域命名，例如建任务、启动 turn、报告 `done`、验收。`admit / claim / finish` 不作为 API 名称。
- 每条命令写明五项：
  1. 调用者与权限；
  2. 前置条件；
  3. 同一事务内写入的内容；
  4. 幂等键；
  5. 经 outbox 执行的副作用。
- 命令清单待定，见 §5。

## 2. 占用

占用分两层。每层的资源在同一个事务中一次取得。

| 层 | 持有者 | 资源 | 取得 | 释放 |
|---|---|---|---|---|
| 任务层 | 任务 | role 及其槽位 | 任务开始执行时 | 任务完成或放弃时 |
| turn 层 | turn | native session 的运行权 | 登记 turn 时 | turn 结束时。结果为 unknown 时，对账结束后才释放 |

v1 中 role 与槽位一一固定，因此任务层不会出现"取得了 role，却没有取得工作目录"的半占用（[#1](https://github.com/vorton-lang/Lobotomy/issues/1)）。Reviewer 没有固定槽位，审查现场在固定的候选成果上单独物化（harness-adapter.md §3）。

### 唯一约束

以下约束由数据库强制，不依赖调用方自行检查：

- 每个 task 最多一个未关闭的 attempt。
- 每个 role 最多被一个任务占用。
- 每个 native session 最多一个未结束的 turn。已登记、运行中、unknown 都算未结束。

## 3. turn 与消息投递

### 3.1 turn

- 一个 turn 就是一次 CLI 进程运行（harness-adapter.md §1.1）。
- 运行时在启动 CLI 前登记 turn，生成 turn_id（harness-adapter.md §1.3 第 8 条）。
- GUI 的 Thread → Turn → Item 中，Turn 使用同一个 turn_id，不另设编号（frontend.md §3）。
- **MCP token 按 turn 发放。**
  - 每个 turn 本来就要重新传 MCP 配置（harness-adapter.md §1.3 第 2 条），按 turn 发放 token 没有额外的调用成本。
  - token 与发放它的 turn 永久对应。`done` 等调用因此直接绑定到 turn，不按"角色当前任务"反查归属。
  - turn 结束后，如果旧进程仍用该 token 调用，后端把调用记为迟到。迟到的调用不能推进状态。
  - URL 不出现在 prompt 中，因此预期不影响 prompt cache。这一点尚未实测：现有的跨进程缓存实测使用的是固定 URL。

### 3.2 turn 的状态

| 状态 | 进入条件 |
|---|---|
| 已登记 | 运行时在同一个事务中写入 turn 记录、MCP token 和绑定的输入消息。CLI 尚未启动 |
| 运行中 | CLI 已启动。运行时记下 pid 与进程启动时间，用于在 pid 被复用时区分进程 |
| 已结束 · completed | 运行时收到 harness 的 turn 结束事件（Claude `result`，Codex `turn.completed`），且 CLI 已退出 |
| 已结束 · failed | harness 报告错误，例如额度被拒、登录失效、参数错误。按系统故障通知（manager-actions.md §6） |
| 已结束 · interrupted | CLI 已退出，但没有 turn 结束事件。原因包括停止或中断、CLI 崩溃，以及对账结果 |
| unknown | 后端重启时，处于"已登记"或"运行中"的 turn 都改为 unknown |

completed 只表示 turn 正常结束，不表示任务完成。任务完成取决于 `done` 与验收（harness-adapter.md §4.1）。

### 3.3 对账

"待对账"只核对一件事：CLI 进程是否已经退出。

只核对这一件事的理由：

- `done` 等业务命令在发生时已写入 SQLite；对话 item 在完成时已落库。
- Lobotomy 不恢复执行现场（harness-adapter.md §1.7），因此不需要从 harness 的会话文件重建业务事实。

核对方法：

1. 运行时按记下的 pid 与进程启动时间检查进程。
2. 如果后端在记下 pid 之前已崩溃，运行时按命令行中的 turn 标识查找进程。Codex 的 MCP URL 在命令行参数中；Claude 的 MCP 配置文件按 turn_id 命名。

核对结果：

- **CLI 已退出**：turn 记为 interrupted，运行时释放 native session 的运行权。
  - 已有 `done` 的采集意图时，运行时照常采集（harness-adapter.md §4.1）。
  - 没有采集意图时，槽位保持原样，等用户选择继续、新建 native session 或放弃（harness-adapter.md §3）。
- **CLI 仍在运行**（例如 Linux 上 harness 没有响应 SIGTERM）：turn 保持 unknown，运行时定期检查进程是否退出。GUI 显示"上一个 turn 的进程仍在运行"，并提供"终止"。运行时不自动终止进程。

### 3.4 消息投递

| 状态 | 判断依据 |
|---|---|
| 排队 | 消息在 role 的收件箱中，尚未绑定到 turn |
| 已绑定 | 与 turn 登记在同一个事务中写入 |
| 已投递 | CLI 已启动，stdin 写入并关闭成功 |

- 消息投递后，跟随所在 turn 的结果：completed、failed、interrupted 或 unknown。
- "已处理"不作为机器状态。模型是否按消息行事属于语义判断；机器能确认的只有"投递它的 turn 是否正常结束"。
- 输入是否已进入 harness 自己的会话记录，需要实测（harness-adapter.md §6）。
- **所在 turn 未正常结束时，消息不自动重投。** resume 接续的是原来的 native session，其记录中可能已有这条消息；自动重投可能造成重复。GUI 把这些消息标为"所在 turn 未正常结束"，由用户决定继续还是重发。
- 因额度失败的 turn 如何处理，见 §5 第 3 项。

## 4. 任务流（M1）

### 4.1 attempt

- 每次进入执行阶段，运行时开一个新的 attempt。进入执行的情况有四种：任务开始、验证失败、退回、重开。
- attempt 内每次运行 CLI 是一个 turn。同一个 attempt 内接续时，运行时只新建 turn，不新建 attempt。例如：执行者提问后用户回答，turn 被中断后用户选择继续，更换 native session 后的第一个 turn。
- 执行者报告 `done` 或任务被放弃时，attempt 结束。之后的验证与验收针对这个 attempt 产出的候选成果。

turn 是执行单位：一个进程、一个 MCP token，对账按 turn 进行。attempt 是业务单位：一次执行交出一份候选成果，证据与验收针对它。

### 4.2 离开执行阶段后的消息

任务离开执行阶段后（验证、审查、验收），发给执行者的消息进入收件箱排队。任务回到执行或关闭时，运行时再投递。

- 理由：候选成果已经固定。执行者此时如果运行 turn，可能修改槽位，使槽位与候选成果不一致；4.3 的重新物化也可能覆盖这些修改。
- 代价：验收期间不能与执行者对话。用户需要提问时，退回任务并附上问题。

### 4.3 回到执行时的槽位

- 默认：槽位保持原样，执行者接着修改。HEAD 仍是槽位物化时的集成版本，`git diff` 显示整个任务的改动。
- 例外：候选成果在验证时被 rebase 到新的集成版本时，运行时把槽位重新物化为 rebase 后的候选成果。有冲突时，冲突标记写入文件。HEAD 设为新的集成版本。
- 4.2 保证此时槽位内容等于候选成果，因此重新物化不会丢失修改。
- v1 只有一个 Worker，任务进行期间集成版本一般不前进，这个例外很少出现。

### 4.4 修改完成条件

- 用户随时可以修改完成条件。每次修改生成一个新版本，不打断正在运行的 turn。
- 任务在执行阶段时，运行时把新版本作为一条消息投递给执行者。
- 任务在验证或验收阶段时，运行时不自动退回任务，更新消息按 4.2 排队。验收请求带完成条件版本；版本不符时，验收被拒绝。用户按新版本重新判断，再决定验收或退回。
- 已有的证据仍绑定旧版本。

### 4.5 证据何时作废

| 证据 | 绑定 | 作废条件 |
|---|---|---|
| 检查结果 | 成果内容、基线、检查配置 | 三者任一变化。完成条件的文字变化不使其作废 |
| 验收 | 成果、完成条件版本、集成版本头 | 提交时任一项与当前值不符（CAS） |
| 审查结论（M2） | 成果、完成条件版本 | 完成条件版本变化。基线变化后能否沿用，M2 再定 |

以下变化不使证据作废：role 下线后再上线、更换 native session、展示字段变化。

### 4.6 重开

- 只能重开已关闭的任务，即已完成或已放弃的任务。重开建立新的 attempt，任务进入排队。
- 代码起点：最后一份候选成果 rebase 到当前集成版本；没有候选成果时，从当前集成版本开始。对已完成的任务，两者相同。
- 继承：任务 ID、当前完成条件版本、审查预算（不重置，roles-and-tasks.md §2.2）。
- 不继承：检查结果、审查结论。
- native session 按 role 管理，重开时不更换。轮换仍由用户手动进行。

## 5. 未定

按讨论顺序：

1. 副作用与回执：启动 CLI、物化槽位、物化预览、通知。
2. 对话记录模型：Thread 的单位、存储、序号、引用 ID。
3. 额度在 M1 的最小行为（[#4](https://github.com/vorton-lang/Lobotomy/issues/4)）。
4. M1 的对象集合与命令清单：以上各项定下后汇总。
