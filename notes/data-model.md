# 数据模型：业务对象、命令与对话记录

> Status: 设计中（2026-10-03）
> 按 [#7](https://github.com/vorton-lang/Lobotomy/issues/7) 逐项讨论；已确认的内容写入本文，未讨论的列在 §4。
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
- 命令清单待定，见 §4。

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

## 3. turn

- 一个 turn 就是一次 CLI 进程运行（harness-adapter.md §1.1）。
- 运行时在启动 CLI 前登记 turn，生成 turn_id（harness-adapter.md §1.3 第 8 条）。
- GUI 的 Thread → Turn → Item 中，Turn 使用同一个 turn_id，不另设编号（frontend.md §3）。
- **MCP token 按 turn 发放。**
  - 每个 turn 本来就要重新传 MCP 配置（harness-adapter.md §1.3 第 2 条），按 turn 发放 token 没有额外的调用成本。
  - token 与发放它的 turn 永久对应。`done` 等调用因此直接绑定到 turn，不按"角色当前任务"反查归属。
  - turn 结束后，如果旧进程仍用该 token 调用，后端把调用记为迟到。迟到的调用不能推进状态。
  - URL 不出现在 prompt 中，因此预期不影响 prompt cache。这一点尚未实测：现有的跨进程缓存实测使用的是固定 URL。

## 4. 未定

按讨论顺序：

1. turn 的生命周期与对账：turn 的状态，"待对账"核对什么，消息的"已投递 / 已处理"如何区分。
2. M1 的任务流：完成条件被修改，退回后的 attempt 与槽位基线，重开时继承什么，证据何时重新验证。
3. 副作用与回执：启动 CLI、物化槽位、物化预览、通知。
4. 对话记录模型：Thread 的单位、存储、序号、引用 ID。
5. 额度在 M1 的最小行为（[#4](https://github.com/vorton-lang/Lobotomy/issues/4)）。
6. M1 的对象集合与命令清单：以上各项定下后汇总。
